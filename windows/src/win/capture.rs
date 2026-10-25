//! DXGI Desktop Duplication of one output, GPU conversion to NV12 with the D3D11 video processor,
//! and the cursor: composited into the frame (duplication frames never contain it) or reported to
//! the client as CURSOR_POS / CURSOR_SHAPE when it draws the cursor itself.

use super::monitors::Monitor;
use crate::cursor::CursorImage;
use crate::platform::FrameSink;
use crate::proto::messages::{CursorPos, CursorShape, Message};
use anyhow::{anyhow, Context};
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

/// NV12 surfaces in rotation, so the encoder can still read a frame while the next is converted.
const NV12_RING: usize = 6;
/// Largest cursor drawn into frames (staging texture size).
const CURSOR_MAX: u32 = 256;

const POINTER_MONOCHROME: u32 = 1;
const POINTER_COLOR: u32 = 2;
const POINTER_MASKED_COLOR: u32 = 4;

pub enum Captured {
    /// The desktop or the cursor changed; the NV12 frame is ready.
    Frame(ID3D11Texture2D),
    Unchanged,
}

pub struct Capture {
    pub device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    dup: Option<IDXGIOutputDuplication>,
    /// Duplication surface size (unrotated scan-out orientation).
    src_w: u32,
    src_h: u32,
    rotation: DXGI_MODE_ROTATION,
    out_w: u32,
    out_h: u32,
    fps: u32,
    /// Clean copy of the desktop and the copy the cursor is drawn into.
    desktop: Option<ID3D11Texture2D>,
    composed: Option<ID3D11Texture2D>,
    cursor_staging: Option<ID3D11Texture2D>,
    have_image: bool,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    vp_enum: Option<ID3D11VideoProcessorEnumerator>,
    vp: Option<ID3D11VideoProcessor>,
    input_view: Option<ID3D11VideoProcessorInputView>,
    ring: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    ring_next: usize,
    // Cursor
    draw_cursor: bool,
    client_cursor: Option<FrameSink>,
    cursor_visible: bool,
    /// Top-left of the cursor image in output coordinates.
    cursor_pos: (i32, i32),
    cursor_shape: Option<CursorImage>,
    shape_buf: Vec<u8>,
    sent_pos: Option<CursorPos>,
}

impl Capture {
    pub fn new(
        monitor: &Monitor,
        out_w: u32,
        out_h: u32,
        fps: u32,
        draw_cursor: bool,
        client_cursor: Option<FrameSink>,
    ) -> anyhow::Result<Capture> {
        let (device, context) = create_device(&monitor.adapter)?;
        let video_device: ID3D11VideoDevice = device.cast().context("D3D11 video device")?;
        let video_context: ID3D11VideoContext = context.cast().context("D3D11 video context")?;
        // Media Foundation uses the device from its own threads.
        if let Ok(mt) = device.cast::<ID3D11Multithread>() {
            unsafe {
                let _ = mt.SetMultithreadProtected(true);
            }
        }
        let mut c = Capture {
            device,
            context,
            output: monitor.output.clone(),
            dup: None,
            src_w: 0,
            src_h: 0,
            rotation: monitor.rotation,
            out_w,
            out_h,
            fps,
            desktop: None,
            composed: None,
            cursor_staging: None,
            have_image: false,
            video_device,
            video_context,
            vp_enum: None,
            vp: None,
            input_view: None,
            ring: Vec::new(),
            ring_next: 0,
            draw_cursor,
            client_cursor,
            cursor_visible: false,
            cursor_pos: (0, 0),
            cursor_shape: None,
            shape_buf: Vec::new(),
            sent_pos: None,
        };
        c.duplicate()?;
        Ok(c)
    }

    /// (Re)creates the duplication and, when the desktop size changed, the conversion resources.
    fn duplicate(&mut self) -> anyhow::Result<()> {
        self.dup = None;
        let dup =
            unsafe { self.output.DuplicateOutput(&self.device) }.context("DuplicateOutput")?;
        let desc = unsafe { dup.GetDesc() };
        let (w, h) = (desc.ModeDesc.Width, desc.ModeDesc.Height);
        self.rotation = desc.Rotation;
        if (w, h) != (self.src_w, self.src_h) || self.vp.is_none() {
            self.src_w = w;
            self.src_h = h;
            self.create_resources()?;
        }
        self.dup = Some(dup);
        Ok(())
    }

    fn texture(
        &self,
        w: u32,
        h: u32,
        format: DXGI_FORMAT,
        bind: D3D11_BIND_FLAG,
        usage: D3D11_USAGE,
        cpu: u32,
    ) -> anyhow::Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: usage,
            BindFlags: bind.0 as u32,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
        };
        let mut t = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut t)) }?;
        t.ok_or_else(|| anyhow!("CreateTexture2D returned nothing"))
    }

    fn create_resources(&mut self) -> anyhow::Result<()> {
        let (w, h) = (self.src_w, self.src_h);
        let bind = D3D11_BIND_FLAG(D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0);
        self.desktop = Some(self.texture(
            w,
            h,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            bind,
            D3D11_USAGE_DEFAULT,
            0,
        )?);
        let composed = self.texture(
            w,
            h,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            bind,
            D3D11_USAGE_DEFAULT,
            0,
        )?;
        self.cursor_staging = Some(self.texture(
            CURSOR_MAX,
            CURSOR_MAX,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            D3D11_BIND_FLAG(0),
            D3D11_USAGE_STAGING,
            (D3D11_CPU_ACCESS_READ.0 | D3D11_CPU_ACCESS_WRITE.0) as u32,
        )?);

        let rate = DXGI_RATIONAL {
            Numerator: self.fps,
            Denominator: 1,
        };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: w,
            InputHeight: h,
            OutputFrameRate: rate,
            OutputWidth: self.out_w,
            OutputHeight: self.out_h,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        let vp_enum = unsafe { self.video_device.CreateVideoProcessorEnumerator(&content) }
            .context("video processor enumerator")?;
        let vp = unsafe { self.video_device.CreateVideoProcessor(&vp_enum, 0) }
            .context("video processor")?;

        let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut input_view = None;
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                &composed,
                &vp_enum,
                &in_desc,
                Some(&mut input_view),
            )
        }
        .context("video processor input view")?;

        let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut ring = Vec::with_capacity(NV12_RING);
        for _ in 0..NV12_RING {
            let nv12 = self.texture(
                self.out_w,
                self.out_h,
                DXGI_FORMAT_NV12,
                D3D11_BIND_RENDER_TARGET,
                D3D11_USAGE_DEFAULT,
                0,
            )?;
            let mut view = None;
            unsafe {
                self.video_device.CreateVideoProcessorOutputView(
                    &nv12,
                    &vp_enum,
                    &out_desc,
                    Some(&mut view),
                )
            }
            .context("video processor output view (NV12)")?;
            ring.push((nv12, view.ok_or_else(|| anyhow!("no output view"))?));
        }

        unsafe {
            let vc = &self.video_context;
            vc.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            vc.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);
            let src = RECT {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            };
            let dst = RECT {
                left: 0,
                top: 0,
                right: self.out_w as i32,
                bottom: self.out_h as i32,
            };
            vc.VideoProcessorSetStreamSourceRect(&vp, 0, true, Some(&src));
            vc.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&dst));
            vc.VideoProcessorSetOutputTargetRect(&vp, true, Some(&dst));
            let rotation = match self.rotation {
                DXGI_MODE_ROTATION_ROTATE90 => Some(D3D11_VIDEO_PROCESSOR_ROTATION_90),
                DXGI_MODE_ROTATION_ROTATE180 => Some(D3D11_VIDEO_PROCESSOR_ROTATION_180),
                DXGI_MODE_ROTATION_ROTATE270 => Some(D3D11_VIDEO_PROCESSOR_ROTATION_270),
                _ => None,
            };
            if let Some(r) = rotation {
                vc.VideoProcessorSetStreamRotation(&vp, 0, true, r);
            }
            // Full-range sRGB in, BT.709 limited-range YUV out (what the encoder signals).
            if let Ok(vc1) = vc.cast::<ID3D11VideoContext1>() {
                vc1.VideoProcessorSetStreamColorSpace1(
                    &vp,
                    0,
                    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                );
                vc1.VideoProcessorSetOutputColorSpace1(
                    &vp,
                    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                );
            }
        }
        self.composed = Some(composed);
        self.input_view = input_view;
        self.vp_enum = Some(vp_enum);
        self.vp = Some(vp);
        self.ring = ring;
        self.have_image = false;
        Ok(())
    }

    /// Waits up to `timeout_ms` for a desktop update. With `force`, converts the last image again
    /// even if nothing changed (keyframe on a static screen).
    pub fn capture(&mut self, timeout_ms: u32, force: bool) -> anyhow::Result<Captured> {
        let mut changed = false;
        if self.dup.is_none() {
            // Lost to a mode change or the secure desktop; retry until it comes back.
            if self.duplicate().is_err() {
                std::thread::sleep(std::time::Duration::from_millis(timeout_ms as u64));
                return Ok(Captured::Unchanged);
            }
        }
        let dup = self.dup.clone().expect("duplication");
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match unsafe { dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) } {
            Ok(()) => {
                let r = self.on_frame(&dup, &info, resource);
                unsafe {
                    let _ = dup.ReleaseFrame();
                }
                changed = r?;
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST || e.code() == DXGI_ERROR_INVALID_CALL => {
                crate::debug!("desktop duplication lost ({e}); recreating");
                self.dup = None;
                return Ok(Captured::Unchanged);
            }
            Err(e) => return Err(e).context("AcquireNextFrame"),
        }
        if !self.have_image || !(changed || force) {
            return Ok(Captured::Unchanged);
        }
        Ok(Captured::Frame(self.convert()?))
    }

    /// Returns whether the picture to encode changed.
    fn on_frame(
        &mut self,
        dup: &IDXGIOutputDuplication,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<IDXGIResource>,
    ) -> anyhow::Result<bool> {
        let mut changed = false;
        let mut cursor_changed = false;
        if info.LastMouseUpdateTime != 0 {
            let p = info.PointerPosition;
            let pos = (p.Position.x, p.Position.y);
            if p.Visible.as_bool() != self.cursor_visible || pos != self.cursor_pos {
                cursor_changed = true;
            }
            self.cursor_visible = p.Visible.as_bool();
            self.cursor_pos = pos;
        }
        if info.PointerShapeBufferSize > 0 {
            self.shape_buf
                .resize(info.PointerShapeBufferSize as usize, 0);
            let mut required = 0u32;
            let mut si = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            let ok = unsafe {
                dup.GetFramePointerShape(
                    self.shape_buf.len() as u32,
                    self.shape_buf.as_mut_ptr().cast(),
                    &mut required,
                    &mut si,
                )
            };
            if ok.is_ok() {
                let hot = (si.HotSpot.x.max(0) as u32, si.HotSpot.y.max(0) as u32);
                let pitch = si.Pitch as usize;
                self.cursor_shape = match si.Type {
                    POINTER_MONOCHROME => CursorImage::from_monochrome(
                        si.Width,
                        si.Height,
                        pitch,
                        &self.shape_buf,
                        hot,
                    ),
                    POINTER_COLOR => {
                        CursorImage::from_color(si.Width, si.Height, pitch, &self.shape_buf, hot)
                    }
                    POINTER_MASKED_COLOR => CursorImage::from_masked_color(
                        si.Width,
                        si.Height,
                        pitch,
                        &self.shape_buf,
                        hot,
                    ),
                    _ => None,
                };
                cursor_changed = true;
                self.send_cursor_shape();
            }
        }
        if cursor_changed {
            self.send_cursor_pos();
            changed |= self.draw_cursor;
        }
        if info.LastPresentTime != 0 {
            if let Some(res) = resource {
                let tex: ID3D11Texture2D = res.cast()?;
                unsafe {
                    self.context
                        .CopyResource(self.desktop.as_ref().unwrap(), &tex)
                };
                self.have_image = true;
                changed = true;
            }
        }
        Ok(changed)
    }

    /// Output pixels per duplication pixel.
    fn scale(&self) -> f64 {
        let src_w = if matches!(
            self.rotation,
            DXGI_MODE_ROTATION_ROTATE90 | DXGI_MODE_ROTATION_ROTATE270
        ) {
            self.src_h
        } else {
            self.src_w
        };
        self.out_w as f64 / src_w.max(1) as f64
    }

    fn send_cursor_shape(&mut self) {
        let (Some(sink), Some(shape)) = (&self.client_cursor, &self.cursor_shape) else {
            return;
        };
        let s = shape.scaled(self.scale());
        if s.w > 256 || s.h > 256 {
            return;
        }
        sink(
            CursorShape {
                w: s.w as u16,
                h: s.h as u16,
                hot_x: s.hot_x as u16,
                hot_y: s.hot_y as u16,
                rgba: s.client_rgba(),
            }
            .frame(),
        );
    }

    fn send_cursor_pos(&mut self) {
        let Some(sink) = &self.client_cursor else {
            return;
        };
        let (hx, hy) = self
            .cursor_shape
            .as_ref()
            .map(|s| (s.hot_x as i32, s.hot_y as i32))
            .unwrap_or((0, 0));
        let k = self.scale();
        let pos = CursorPos {
            x: (((self.cursor_pos.0 + hx) as f64) * k)
                .round()
                .clamp(i16::MIN as f64, i16::MAX as f64) as i16,
            y: (((self.cursor_pos.1 + hy) as f64) * k)
                .round()
                .clamp(i16::MIN as f64, i16::MAX as f64) as i16,
            visible: self.cursor_visible as u8,
        };
        if self.sent_pos.as_ref() == Some(&pos) {
            return;
        }
        // While hidden, one update is enough.
        if pos.visible == 0 && self.sent_pos.as_ref().is_some_and(|p| p.visible == 0) {
            return;
        }
        self.sent_pos = Some(pos.clone());
        sink(pos.frame());
    }

    /// Copies the desktop, draws the cursor, converts to the next NV12 surface.
    fn convert(&mut self) -> anyhow::Result<ID3D11Texture2D> {
        let (desktop, composed) = (
            self.desktop.clone().unwrap(),
            self.composed.clone().unwrap(),
        );
        unsafe { self.context.CopyResource(&composed, &desktop) };
        // Rotated outputs keep the cursor out of the frame: its position is in rotated coordinates.
        if self.draw_cursor
            && self.cursor_visible
            && self.rotation.0 <= DXGI_MODE_ROTATION_IDENTITY.0
        {
            if let Err(e) = self.composite_cursor(&composed) {
                crate::debug!("cursor compositing failed: {e}");
            }
        }
        let (nv12, view) = self.ring[self.ring_next].clone();
        self.ring_next = (self.ring_next + 1) % self.ring.len();
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(self.input_view.clone()),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };
        let streams = [stream];
        let r = unsafe {
            self.video_context
                .VideoProcessorBlt(self.vp.as_ref().unwrap(), &view, 0, &streams)
        };
        let [mut s] = streams;
        unsafe { std::mem::ManuallyDrop::drop(&mut s.pInputSurface) };
        r.context("VideoProcessorBlt")?;
        Ok(nv12)
    }

    /// CPU blend through a small staging texture: copy the region under the cursor, draw, copy back.
    fn composite_cursor(&mut self, target: &ID3D11Texture2D) -> anyhow::Result<()> {
        let Some(shape) = &self.cursor_shape else {
            return Ok(());
        };
        let staging = self.cursor_staging.clone().unwrap();
        let (x0, y0) = (
            self.cursor_pos.0.max(0) as u32,
            self.cursor_pos.1.max(0) as u32,
        );
        let x1 = ((self.cursor_pos.0 + shape.w as i32).max(0) as u32).min(self.src_w);
        let y1 = ((self.cursor_pos.1 + shape.h as i32).max(0) as u32).min(self.src_h);
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let (bw, bh) = ((x1 - x0).min(CURSOR_MAX), (y1 - y0).min(CURSOR_MAX));
        let region = D3D11_BOX {
            left: x0,
            top: y0,
            front: 0,
            right: x0 + bw,
            bottom: y0 + bh,
            back: 1,
        };
        unsafe {
            self.context
                .CopySubresourceRegion(&staging, 0, 0, 0, 0, target, 0, Some(&region));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ_WRITE, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let pixels = std::slice::from_raw_parts_mut(
                mapped.pData as *mut u8,
                pitch * CURSOR_MAX as usize,
            );
            shape.composite_bgra(
                pixels,
                pitch,
                bw,
                bh,
                self.cursor_pos.0 - x0 as i32,
                self.cursor_pos.1 - y0 as i32,
            );
            self.context.Unmap(&staging, 0);
            let back = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: bw,
                bottom: bh,
                back: 1,
            };
            self.context
                .CopySubresourceRegion(target, 0, x0, y0, 0, &staging, 0, Some(&back));
        }
        Ok(())
    }

    pub fn context(&self) -> &ID3D11DeviceContext {
        &self.context
    }
}

fn create_device(adapter: &IDXGIAdapter1) -> anyhow::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
    ];
    let (mut device, mut context) = (None, None);
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .context("D3D11CreateDevice")?;
    Ok((
        device.ok_or_else(|| anyhow!("no D3D11 device"))?,
        context.ok_or_else(|| anyhow!("no D3D11 context"))?,
    ))
}
