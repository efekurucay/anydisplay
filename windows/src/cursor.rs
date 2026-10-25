//! Cursor images from DXGI pointer shapes: straight-alpha RGBA for CURSOR_SHAPE (PROTOCOL.md 5.4)
//! and CPU compositing into captured frames (Desktop Duplication frames never contain the cursor).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    pub w: u32,
    pub h: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    /// Straight-alpha RGBA8.
    pub rgba: Vec<u8>,
    /// Per pixel: RGB to XOR onto the screen (monochrome "invert" and masked-color cursors), or zero.
    pub xor: Vec<[u8; 3]>,
}

impl CursorImage {
    fn blank(w: u32, h: u32, hot_x: u32, hot_y: u32) -> Self {
        let n = (w * h) as usize;
        CursorImage {
            w,
            h,
            hot_x,
            hot_y,
            rgba: vec![0; n * 4],
            xor: vec![[0; 3]; n],
        }
    }

    /// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME`: AND mask rows then XOR mask rows, 1 bpp;
    /// `height` counts both masks.
    pub fn from_monochrome(
        w: u32,
        height: u32,
        pitch: usize,
        data: &[u8],
        hot: (u32, u32),
    ) -> Option<Self> {
        let h = height / 2;
        let mut img = Self::blank(w, h, hot.0, hot.1);
        let bit = |row: u32, x: u32| -> Option<bool> {
            let byte = data.get(row as usize * pitch + (x / 8) as usize)?;
            Some(byte >> (7 - x % 8) & 1 == 1)
        };
        for y in 0..h {
            for x in 0..w {
                let (and, xor) = (bit(y, x)?, bit(y + h, x)?);
                let i = (y * w + x) as usize;
                match (and, xor) {
                    (false, false) => img.rgba[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 255]),
                    (false, true) => {
                        img.rgba[i * 4..i * 4 + 4].copy_from_slice(&[255, 255, 255, 255])
                    }
                    (true, false) => {}
                    (true, true) => img.xor[i] = [255, 255, 255],
                }
            }
        }
        Some(img)
    }

    /// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR`: BGRA with alpha.
    pub fn from_color(w: u32, h: u32, pitch: usize, data: &[u8], hot: (u32, u32)) -> Option<Self> {
        let mut img = Self::blank(w, h, hot.0, hot.1);
        for y in 0..h {
            for x in 0..w {
                let s = y as usize * pitch + x as usize * 4;
                let p = data.get(s..s + 4)?;
                let i = (y * w + x) as usize * 4;
                img.rgba[i..i + 4].copy_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        Some(img)
    }

    /// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR`: BGRX where the top byte is a mask:
    /// 0 = replace the screen pixel, 0xFF = XOR it.
    pub fn from_masked_color(
        w: u32,
        h: u32,
        pitch: usize,
        data: &[u8],
        hot: (u32, u32),
    ) -> Option<Self> {
        let mut img = Self::blank(w, h, hot.0, hot.1);
        for y in 0..h {
            for x in 0..w {
                let s = y as usize * pitch + x as usize * 4;
                let p = data.get(s..s + 4)?;
                let i = (y * w + x) as usize;
                if p[3] == 0 {
                    img.rgba[i * 4..i * 4 + 4].copy_from_slice(&[p[2], p[1], p[0], 255]);
                } else {
                    img.xor[i] = [p[2], p[1], p[0]];
                }
            }
        }
        Some(img)
    }

    /// RGBA for clients, which cannot XOR: inverting pixels become opaque black.
    pub fn client_rgba(&self) -> Vec<u8> {
        let mut out = self.rgba.clone();
        for (i, x) in self.xor.iter().enumerate() {
            if *x != [0; 3] {
                out[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        out
    }

    /// Nearest-neighbour resize (cursor shapes are sent at stream scale).
    pub fn scaled(&self, factor: f64) -> CursorImage {
        let w = ((self.w as f64 * factor).round() as u32).max(1);
        let h = ((self.h as f64 * factor).round() as u32).max(1);
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        let mut out = Self::blank(w, h, 0, 0);
        out.hot_x = ((self.hot_x as f64 * factor) as u32).min(w - 1);
        out.hot_y = ((self.hot_y as f64 * factor) as u32).min(h - 1);
        for y in 0..h {
            let sy = (y as u64 * self.h as u64 / h as u64) as u32;
            for x in 0..w {
                let sx = (x as u64 * self.w as u64 / w as u64) as u32;
                let (d, s) = ((y * w + x) as usize, (sy * self.w + sx) as usize);
                out.rgba[d * 4..d * 4 + 4].copy_from_slice(&self.rgba[s * 4..s * 4 + 4]);
                out.xor[d] = self.xor[s];
            }
        }
        out
    }

    /// Draws the cursor with its top-left corner at (`left`, `top`) into a BGRA8 image.
    pub fn composite_bgra(
        &self,
        dst: &mut [u8],
        pitch: usize,
        width: u32,
        height: u32,
        left: i32,
        top: i32,
    ) {
        for y in 0..self.h as i32 {
            let dy = top + y;
            if dy < 0 || dy >= height as i32 {
                continue;
            }
            for x in 0..self.w as i32 {
                let dx = left + x;
                if dx < 0 || dx >= width as i32 {
                    continue;
                }
                let i = (y as u32 * self.w + x as u32) as usize;
                let d = dy as usize * pitch + dx as usize * 4;
                let Some(px) = dst.get_mut(d..d + 4) else {
                    continue;
                };
                let xor = self.xor[i];
                if xor != [0; 3] {
                    px[0] ^= xor[2];
                    px[1] ^= xor[1];
                    px[2] ^= xor[0];
                    continue;
                }
                let [r, g, b, a] = [
                    self.rgba[i * 4],
                    self.rgba[i * 4 + 1],
                    self.rgba[i * 4 + 2],
                    self.rgba[i * 4 + 3],
                ];
                if a == 0 {
                    continue;
                }
                let a = a as u32;
                let mix = |s: u8, d: u8| ((s as u32 * a + d as u32 * (255 - a) + 127) / 255) as u8;
                px[0] = mix(b, px[0]);
                px[1] = mix(g, px[1]);
                px[2] = mix(r, px[2]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_masks() {
        // 8x1 cursor: AND row then XOR row.
        let data = [0b0011_0000u8, 0b0101_0000];
        let img = CursorImage::from_monochrome(8, 2, 1, &data, (0, 0)).unwrap();
        assert_eq!(&img.rgba[0..4], &[0, 0, 0, 255]); // AND0 XOR0
        assert_eq!(&img.rgba[4..8], &[255, 255, 255, 255]); // AND0 XOR1
        assert_eq!(img.rgba[11], 0); // AND1 XOR0 transparent
        assert_eq!(img.xor[3], [255; 3]); // AND1 XOR1 invert
        assert_eq!(&img.client_rgba()[12..16], &[0, 0, 0, 255]);
    }

    #[test]
    fn color_and_composite() {
        // 1x1 half-transparent red (BGRA).
        let img = CursorImage::from_color(1, 1, 4, &[0, 0, 255, 128], (0, 0)).unwrap();
        assert_eq!(img.rgba, vec![255, 0, 0, 128]);
        let mut dst = vec![0u8, 0, 0, 255, 255, 255, 255, 255]; // two BGRA pixels
        img.composite_bgra(&mut dst, 8, 2, 1, 1, 0);
        assert_eq!(&dst[4..8], &[127, 127, 255, 255]);
        assert_eq!(&dst[0..4], &[0, 0, 0, 255]);
        // Masked color XOR.
        let m = CursorImage::from_masked_color(1, 1, 4, &[0xff, 0, 0, 0xff], (0, 0)).unwrap();
        let mut px = vec![0x0f, 0x0f, 0x0f, 0xff];
        m.composite_bgra(&mut px, 4, 1, 1, 0, 0);
        assert_eq!(px, vec![0xf0, 0x0f, 0x0f, 0xff]);
        // Clipped off-screen draws nothing.
        img.composite_bgra(&mut px, 4, 1, 1, -5, -5);
    }

    #[test]
    fn scaling_keeps_hotspot_inside() {
        let img = CursorImage::from_color(4, 4, 16, &[255; 64], (3, 3)).unwrap();
        let s = img.scaled(0.5);
        assert_eq!((s.w, s.h, s.hot_x, s.hot_y), (2, 2, 1, 1));
    }
}
