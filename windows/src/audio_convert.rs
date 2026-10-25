//! Converts captured audio to the stream format: PCM s16le, 48 kHz, stereo (PROTOCOL.md 5.3).

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    F32,
    S16,
    /// 32-bit container (24- or 32-bit valid bits, left-aligned).
    S32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputFormat {
    pub format: SampleFormat,
    pub channels: u16,
    pub rate: u32,
}

/// Stateful converter: channel mapping plus linear resampling that is continuous across buffers.
pub struct AudioConverter {
    input: InputFormat,
    /// Position of the next output sample in input frames, relative to `prev`.
    pos: f64,
    prev: [f32; 2],
}

impl AudioConverter {
    pub fn new(input: InputFormat) -> Self {
        AudioConverter {
            input,
            pos: 0.0,
            prev: [0.0; 2],
        }
    }

    /// Converts interleaved `data` (whole frames) and returns s16le stereo bytes.
    pub fn convert(&mut self, data: &[u8]) -> Vec<u8> {
        let frames = self.stereo_frames(data);
        let mut out = Vec::new();
        if self.input.rate == SAMPLE_RATE {
            for f in &frames {
                push(&mut out, *f);
            }
            return out;
        }
        // Frame k of this buffer is at index k + 1, `prev` at index 0.
        let step = self.input.rate as f64 / SAMPLE_RATE as f64;
        let at = |i: usize| if i == 0 { self.prev } else { frames[i - 1] };
        while self.pos + 1.0 <= frames.len() as f64 {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let (a, b) = (at(i), at(i + 1));
            push(
                &mut out,
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
            );
            self.pos += step;
        }
        if let Some(last) = frames.last() {
            self.prev = *last;
            self.pos -= frames.len() as f64;
        }
        out
    }

    fn stereo_frames(&self, data: &[u8]) -> Vec<[f32; 2]> {
        let bytes = match self.input.format {
            SampleFormat::S16 => 2,
            SampleFormat::F32 | SampleFormat::S32 => 4,
        };
        let ch = self.input.channels.max(1) as usize;
        data.chunks_exact(bytes * ch)
            .map(|frame| {
                let sample = |c: usize| -> f32 {
                    let b = &frame[c * bytes..(c + 1) * bytes];
                    match self.input.format {
                        SampleFormat::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                        SampleFormat::S16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
                        SampleFormat::S32 => {
                            i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0
                        }
                    }
                };
                // Mono is duplicated; beyond two channels, front left/right are kept.
                if ch == 1 {
                    [sample(0); 2]
                } else {
                    [sample(0), sample(1)]
                }
            })
            .collect()
    }
}

fn push(out: &mut Vec<u8>, f: [f32; 2]) {
    for s in f {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s16(out: &[u8]) -> Vec<i16> {
        out.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()
    }

    #[test]
    fn float_surround_to_s16_stereo() {
        let mut c = AudioConverter::new(InputFormat {
            format: SampleFormat::F32,
            channels: 6,
            rate: 48_000,
        });
        let frame: Vec<u8> = [0.5f32, -1.0, 0.9, 0.9, 0.9, 0.9]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(s16(&c.convert(&frame)), vec![16384, -32767]);
    }

    #[test]
    fn mono_s16_duplicates() {
        let mut c = AudioConverter::new(InputFormat {
            format: SampleFormat::S16,
            channels: 1,
            rate: 48_000,
        });
        assert_eq!(s16(&c.convert(&1000i16.to_le_bytes())), vec![1000, 1000]);
    }

    #[test]
    fn resampling_keeps_rate_across_buffers() {
        let mut c = AudioConverter::new(InputFormat {
            format: SampleFormat::S16,
            channels: 2,
            rate: 44_100,
        });
        let one_tenth: Vec<u8> = (0..4410).flat_map(|_| [0u8, 0, 0, 0]).collect();
        let total: usize = (0..10).map(|_| c.convert(&one_tenth).len() / 4).sum();
        assert!((47_990..=48_010).contains(&total), "{total}");
    }
}
