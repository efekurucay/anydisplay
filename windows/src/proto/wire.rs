//! Little-endian primitives of PROTOCOL.md section 1.

use std::fmt;

/// A body ended before a field was complete, or a `str` was not UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub &'static str);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed message: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

#[derive(Default)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn with_capacity(n: usize) -> Self {
        Self(Vec::with_capacity(n))
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }

    /// `u16 byteLength` + UTF-8. Longer strings are cut at a character boundary below 65536 bytes.
    pub fn str(&mut self, s: &str) -> &mut Self {
        let mut end = s.len().min(u16::MAX as usize);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.u16(end as u16).raw(&s.as_bytes()[..end])
    }

    pub fn blob(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32).raw(v)
    }

    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

pub struct Reader<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, o: 0 }
    }

    pub fn raw(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.b.len() - self.o < n {
            return Err(DecodeError("short body"));
        }
        let v = &self.b[self.o..self.o + n];
        self.o += n;
        Ok(v)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        Ok(self.raw(N)?.try_into().expect("length checked"))
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.raw(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn i16(&mut self) -> Result<i16, DecodeError> {
        Ok(i16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    pub fn str(&mut self) -> Result<String, DecodeError> {
        let n = self.u16()? as usize;
        String::from_utf8(self.raw(n)?.to_vec()).map_err(|_| DecodeError("str is not UTF-8"))
    }

    pub fn blob(&mut self) -> Result<&'a [u8], DecodeError> {
        let n = self.u32()? as usize;
        self.raw(n)
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let v = &self.b[self.o..];
        self.o = self.b.len();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_little_endian() {
        let b = Writer::new()
            .u8(1)
            .u16(0x0203)
            .i16(-2)
            .u32(0x0405_0607)
            .u64(9)
            .f32(1.5)
            .str("hé")
            .blob(&[7, 8])
            .finish();
        assert_eq!(&b[..3], &[1, 0x03, 0x02]);
        let mut r = Reader::new(&b);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u16().unwrap(), 0x0203);
        assert_eq!(r.i16().unwrap(), -2);
        assert_eq!(r.u32().unwrap(), 0x0405_0607);
        assert_eq!(r.u64().unwrap(), 9);
        assert_eq!(r.f32().unwrap(), 1.5);
        assert_eq!(r.str().unwrap(), "hé");
        assert_eq!(r.blob().unwrap(), &[7, 8]);
        assert!(r.u8().is_err());
    }

    #[test]
    fn long_str_is_cut_on_char_boundary() {
        let s = "é".repeat(40_000); // 80 000 bytes
        let b = Writer::new().str(&s).finish();
        let n = u16::from_le_bytes([b[0], b[1]]) as usize;
        assert_eq!(n, 65534);
        assert!(std::str::from_utf8(&b[2..]).is_ok());
    }
}
