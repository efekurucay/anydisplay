//! Framing and per-direction AES-256-GCM (PROTOCOL.md 3 and 3.1).

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use std::io::{self, Read};

/// Largest allowed `length` field.
pub const MAX_FRAME_LEN: u32 = 32 * 1024 * 1024;
pub const TAG_LEN: usize = 16;

#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub channel: u8,
    pub ty: u8,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Frame(ch={} type=0x{:02x} len={})",
            self.channel,
            self.ty,
            self.body.len()
        )
    }
}

/// One direction's AEAD state: key plus the frame counter used as nonce.
pub struct Cipher {
    aead: Aes256Gcm,
    seq: u64,
}

impl Cipher {
    pub fn new(key: &[u8; 32]) -> Cipher {
        Cipher {
            aead: Aes256Gcm::new(key.into()),
            seq: 0,
        }
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    fn nonce(seq: u64) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[..8].copy_from_slice(&seq.to_le_bytes());
        n
    }

    /// Ciphertext with the 16-byte tag appended; aad = `channel || type`.
    pub fn seal(&mut self, channel: u8, ty: u8, body: &[u8]) -> Vec<u8> {
        let nonce = Self::nonce(self.seq);
        self.seq += 1;
        self.aead
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: body,
                    aad: &[channel, ty],
                },
            )
            .expect("AES-GCM encryption of an in-memory buffer")
    }

    pub fn open(&mut self, channel: u8, ty: u8, ciphertext: &[u8]) -> Result<Vec<u8>, FrameError> {
        let nonce = Self::nonce(self.seq);
        self.seq += 1;
        self.aead
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &[channel, ty],
                },
            )
            .map_err(|_| FrameError::Authentication)
    }
}

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    /// `length < 2` or above [`MAX_FRAME_LEN`].
    BadLength(u32),
    /// AES-GCM tag mismatch; the connection must close.
    Authentication,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Io(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                write!(f, "connection closed")
            }
            FrameError::Io(e) => write!(f, "{e}"),
            FrameError::BadLength(n) => write!(f, "bad frame length {n}"),
            FrameError::Authentication => write!(f, "frame failed authentication"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e)
    }
}

/// Serializes a frame, encrypting the body when `cipher` is set.
pub fn encode_frame(frame: &Frame, cipher: Option<&mut Cipher>) -> Vec<u8> {
    let sealed;
    let body = match cipher {
        Some(c) => {
            sealed = c.seal(frame.channel, frame.ty, &frame.body);
            &sealed[..]
        }
        None => &frame.body[..],
    };
    let mut out = Vec::with_capacity(6 + body.len());
    out.extend_from_slice(&(body.len() as u32 + 2).to_le_bytes());
    out.push(frame.channel);
    out.push(frame.ty);
    out.extend_from_slice(body);
    out
}

fn finish(
    channel: u8,
    ty: u8,
    body: Vec<u8>,
    cipher: Option<&mut Cipher>,
) -> Result<Frame, FrameError> {
    let body = match cipher {
        Some(c) => c.open(channel, ty, &body)?,
        None => body,
    };
    Ok(Frame { channel, ty, body })
}

fn check_len(len: u32) -> Result<(), FrameError> {
    if !(2..=MAX_FRAME_LEN).contains(&len) {
        return Err(FrameError::BadLength(len));
    }
    Ok(())
}

/// Blocking read of one frame.
pub fn read_frame(r: &mut impl Read, cipher: Option<&mut Cipher>) -> Result<Frame, FrameError> {
    let mut head = [0u8; 6];
    r.read_exact(&mut head)?;
    let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    check_len(len)?;
    let mut body = vec![0u8; len as usize - 2];
    r.read_exact(&mut body)?;
    finish(head[4], head[5], body, cipher)
}

/// Incremental decoder for reads that may time out mid-frame.
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// The next complete frame, if buffered.
    pub fn next(&mut self, cipher: Option<&mut Cipher>) -> Result<Option<Frame>, FrameError> {
        if self.buf.len() < 6 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        check_len(len)?;
        let total = 4 + len as usize;
        if self.buf.len() < total {
            return Ok(None);
        }
        let (channel, ty) = (self.buf[4], self.buf[5]);
        let body = self.buf[6..total].to_vec();
        self.buf.drain(..total);
        finish(channel, ty, body, cipher).map(Some)
    }

    /// Bytes received past the last complete frame.
    pub fn into_remaining(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_round_trip_and_length() {
        let f = Frame {
            channel: 4,
            ty: 0x55,
            body: vec![1, 2, 3],
        };
        let b = encode_frame(&f, None);
        assert_eq!(&b[..6], &[5, 0, 0, 0, 4, 0x55]);
        assert_eq!(read_frame(&mut &b[..], None).unwrap(), f);
    }

    #[test]
    fn rejects_bad_lengths() {
        let b = [1u8, 0, 0, 0, 0];
        assert!(matches!(
            read_frame(&mut &b[..], None),
            Err(FrameError::BadLength(1)) | Err(FrameError::Io(_))
        ));
        let mut d = FrameDecoder::default();
        d.push(&(MAX_FRAME_LEN + 1).to_le_bytes());
        d.push(&[0, 0]);
        assert!(matches!(d.next(None), Err(FrameError::BadLength(_))));
    }

    #[test]
    fn encrypted_round_trip_counts_and_rejects_tamper() {
        let key = [9u8; 32];
        let (mut tx, mut rx) = (Cipher::new(&key), Cipher::new(&key));
        let f = Frame {
            channel: 0,
            ty: 0x13,
            body: vec![7; 12],
        };
        let b1 = encode_frame(&f, Some(&mut tx));
        let b2 = encode_frame(&f, Some(&mut tx));
        assert_eq!(
            u32::from_le_bytes(b1[..4].try_into().unwrap()) as usize,
            2 + 12 + TAG_LEN
        );
        assert_ne!(b1, b2, "nonce advances per frame");
        let mut d = FrameDecoder::default();
        d.push(&b1);
        d.push(&b2[..10]);
        assert_eq!(d.next(Some(&mut rx)).unwrap().unwrap(), f);
        assert!(d.next(Some(&mut rx)).unwrap().is_none());
        d.push(&b2[10..]);
        assert_eq!(d.next(Some(&mut rx)).unwrap().unwrap(), f);
        let mut bad = encode_frame(&f, Some(&mut tx));
        bad[5] ^= 1; // type is authenticated data
        assert!(matches!(
            read_frame(&mut &bad[..], Some(&mut rx)),
            Err(FrameError::Authentication)
        ));
    }
}
