//! Handshake and session cryptography (PROTOCOL.md 3.1 and 4.2).

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("OS random number generator");
    b
}

pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// An ephemeral P-256 key pair.
pub struct KeyPair {
    secret: SecretKey,
    /// Uncompressed X9.62: `0x04 || X || Y`.
    pub public: [u8; 65],
}

impl KeyPair {
    pub fn generate() -> KeyPair {
        loop {
            // A random 32-byte string is a valid scalar unless it is zero or >= n.
            if let Ok(k) = Self::from_private(&random_bytes::<32>()) {
                return k;
            }
        }
    }

    pub fn from_private(bytes: &[u8; 32]) -> Result<KeyPair, p256::elliptic_curve::Error> {
        let secret = SecretKey::from_slice(bytes)?;
        let point = secret.public_key().to_encoded_point(false);
        let public = point
            .as_bytes()
            .try_into()
            .expect("uncompressed P-256 point is 65 bytes");
        Ok(KeyPair { secret, public })
    }

    /// The ECDH shared secret: the 32-byte X coordinate.
    pub fn ecdh(&self, peer_public: &[u8]) -> Result<[u8; 32], p256::elliptic_curve::Error> {
        let peer = PublicKey::from_sec1_bytes(peer_public)?;
        let shared = p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), peer.as_affine());
        Ok((*shared.raw_secret_bytes()).into())
    }
}

/// The 6-digit numeric comparison code of mode 1.
pub fn sas_code(transcript: &[u8; 32]) -> u32 {
    let h = sha256(&[b"anydisplay/1 sas", transcript]);
    u32::from_le_bytes([h[0], h[1], h[2], h[3]]) % 1_000_000
}

pub fn format_sas(code: u32) -> String {
    format!("{code:06}")
}

#[derive(Clone, PartialEq, Eq)]
pub struct SessionKeys {
    pub k_h2c: [u8; 32],
    pub k_c2h: [u8; 32],
    pub k_confirm: [u8; 32],
}

impl std::fmt::Debug for SessionKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionKeys(..)")
    }
}

pub fn key_schedule(z: &[u8; 32], psk: &[u8], transcript: &[u8; 32]) -> SessionKeys {
    let mut ikm = Vec::with_capacity(64);
    ikm.extend_from_slice(z);
    ikm.extend_from_slice(psk);
    let mut okm = [0u8; 96];
    Hkdf::<Sha256>::new(Some(transcript), &ikm)
        .expand(b"anydisplay/1 keys", &mut okm)
        .expect("96 bytes is a valid HKDF-SHA256 length");
    SessionKeys {
        k_h2c: okm[0..32].try_into().unwrap(),
        k_c2h: okm[32..64].try_into().unwrap(),
        k_confirm: okm[64..96].try_into().unwrap(),
    }
}

fn confirm_hmac(k_confirm: &[u8; 32], who: &[u8], transcript: &[u8; 32]) -> HmacSha256 {
    let mut m = HmacSha256::new_from_slice(k_confirm).expect("HMAC takes any key length");
    m.update(who);
    m.update(transcript);
    m
}

/// `HMAC-SHA256(k_confirm, who || transcript)` with `who` = `b"host"` or `b"client"`.
pub fn confirm_mac(k_confirm: &[u8; 32], who: &[u8], transcript: &[u8; 32]) -> [u8; 32] {
    confirm_hmac(k_confirm, who, transcript)
        .finalize()
        .into_bytes()
        .into()
}

/// Constant-time check of a confirmation MAC.
pub fn verify_confirm_mac(
    k_confirm: &[u8; 32],
    who: &[u8],
    transcript: &[u8; 32],
    mac: &[u8],
) -> bool {
    confirm_hmac(k_confirm, who, transcript)
        .verify_slice(mac)
        .is_ok()
}
