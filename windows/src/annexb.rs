//! Annex-B helpers: NAL scanning, keyframe detection, in-band parameter sets (PROTOCOL.md 5.2).

use crate::proto::Codec;

/// Offsets of every start code (`00 00 01` or `00 00 00 01`) and the offset of the NAL header after it.
pub fn nal_units(b: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= b.len() {
        if b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 1 {
            let start = if i > 0 && b[i - 1] == 0 { i - 1 } else { i };
            out.push((start, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

pub fn nal_type(header: u8, codec: Codec) -> u8 {
    match codec {
        Codec::Hevc => (header >> 1) & 0x3f,
        _ => header & 0x1f,
    }
}

pub fn nal_types(au: &[u8], codec: Codec) -> Vec<u8> {
    nal_units(au)
        .into_iter()
        .filter(|&(_, h)| h < au.len())
        .map(|(_, h)| nal_type(au[h], codec))
        .collect()
}

/// IDR (H.264 type 5) or IRAP IDR/CRA (HEVC 19, 20, 21).
pub fn is_keyframe(au: &[u8], codec: Codec) -> bool {
    nal_types(au, codec).iter().any(|&t| match codec {
        Codec::Hevc => (19..=21).contains(&t),
        _ => t == 5,
    })
}

/// SPS and PPS (plus VPS for HEVC) present.
pub fn has_parameter_sets(au: &[u8], codec: Codec) -> bool {
    let t = nal_types(au, codec);
    match codec {
        Codec::Hevc => [32, 33, 34].iter().all(|x| t.contains(x)),
        _ => t.contains(&7) && t.contains(&8),
    }
}

/// Keyframes must carry parameter sets in-band. Encoders that only expose them out of band
/// (Media Foundation's `MF_MT_MPEG_SEQUENCE_HEADER`) get them prepended here.
pub fn with_parameter_sets(au: Vec<u8>, codec: Codec, sequence_header: &[u8]) -> Vec<u8> {
    if sequence_header.is_empty() || has_parameter_sets(&au, codec) {
        return au;
    }
    // Keep a leading access unit delimiter first.
    let aud = if codec == Codec::Hevc { 35 } else { 9 };
    let units = nal_units(&au);
    let insert_at = match units.as_slice() {
        [(_, h), (next, _), ..] if nal_type(au[*h], codec) == aud => *next,
        [(_, h)] if nal_type(au[*h], codec) == aud => au.len(),
        _ => 0,
    };
    let mut out = Vec::with_capacity(au.len() + sequence_header.len());
    out.extend_from_slice(&au[..insert_at]);
    out.extend_from_slice(sequence_header);
    out.extend_from_slice(&au[insert_at..]);
    out
}

/// Splits an Annex-B byte stream into access units at access unit delimiters
/// (the test source runs the encoder with AUDs on, like `anydisplay_ref.py host`).
pub struct AccessUnitSplitter {
    codec: Codec,
    buf: Vec<u8>,
}

impl AccessUnitSplitter {
    pub fn new(codec: Codec) -> Self {
        Self {
            codec,
            buf: Vec::new(),
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn next_au(&mut self) -> Option<Vec<u8>> {
        let aud = if self.codec == Codec::Hevc { 35 } else { 9 };
        let auds: Vec<usize> = nal_units(&self.buf)
            .into_iter()
            .filter(|&(_, h)| h < self.buf.len() && nal_type(self.buf[h], self.codec) == aud)
            .map(|(s, _)| s)
            .collect();
        let (&first, &second) = (auds.first()?, auds.get(1)?);
        let au = self.buf[first..second].to_vec();
        self.buf.drain(..second);
        Some(au)
    }

    /// Whatever follows the last delimiter (the final access unit at end of stream).
    pub fn take_rest(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUD: &[u8] = &[0, 0, 0, 1, 0x09, 0xf0];
    const SPS: &[u8] = &[0, 0, 0, 1, 0x67, 1, 2];
    const PPS: &[u8] = &[0, 0, 0, 1, 0x68, 3];
    const IDR: &[u8] = &[0, 0, 1, 0x65, 0x88, 0x80];
    const P: &[u8] = &[0, 0, 1, 0x41, 0x9a];

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    #[test]
    fn detects_keyframes_and_parameter_sets() {
        let key = cat(&[AUD, SPS, PPS, IDR]);
        assert_eq!(nal_types(&key, Codec::H264), vec![9, 7, 8, 5]);
        assert!(is_keyframe(&key, Codec::H264));
        assert!(has_parameter_sets(&key, Codec::H264));
        assert!(!is_keyframe(&cat(&[AUD, P]), Codec::H264));
        // HEVC: VPS 32, SPS 33, PPS 34, IDR_W_RADL 19.
        let hevc = cat(&[
            &[0, 0, 0, 1, 0x40, 1],
            &[0, 0, 0, 1, 0x42, 1],
            &[0, 0, 0, 1, 0x44, 1],
            &[0, 0, 1, 0x26, 1],
        ]);
        assert!(is_keyframe(&hevc, Codec::Hevc));
        assert!(has_parameter_sets(&hevc, Codec::Hevc));
    }

    #[test]
    fn prepends_sequence_header_after_aud() {
        let header = cat(&[SPS, PPS]);
        assert_eq!(
            with_parameter_sets(cat(&[AUD, IDR]), Codec::H264, &header),
            cat(&[AUD, SPS, PPS, IDR])
        );
        assert_eq!(
            with_parameter_sets(IDR.to_vec(), Codec::H264, &header),
            cat(&[SPS, PPS, IDR])
        );
        let already = cat(&[SPS, PPS, IDR]);
        assert_eq!(
            with_parameter_sets(already.clone(), Codec::H264, &header),
            already
        );
    }

    #[test]
    fn splits_stream_on_delimiters_across_chunks() {
        let stream = cat(&[&[0xde, 0xad], AUD, SPS, PPS, IDR, AUD, P, AUD, P, AUD]);
        let mut s = AccessUnitSplitter::new(Codec::H264);
        let mut out = Vec::new();
        for chunk in stream.chunks(3) {
            s.push(chunk);
            while let Some(au) = s.next_au() {
                out.push(au);
            }
        }
        assert_eq!(
            out,
            vec![cat(&[AUD, SPS, PPS, IDR]), cat(&[AUD, P]), cat(&[AUD, P])]
        );
    }
}
