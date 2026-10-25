import CryptoKit
import Foundation

public enum AnyDisplayCrypto {
    public static func sha256(_ parts: Data...) -> Data {
        var h = SHA256()
        for p in parts { h.update(data: p) }
        return Data(h.finalize())
    }

    /// `SHA-256(body(HELLO_C) || body(HELLO_H) || body(REVEAL))`
    public static func transcript(helloC: Data, helloH: Data, reveal: Data) -> Data {
        sha256(helloC, helloH, reveal)
    }

    /// `u32_le(SHA-256("anydisplay/1 sas" || transcript)[0..4]) mod 1 000 000`
    public static func sasCode(transcript: Data) -> UInt32 {
        let d = sha256(Data("anydisplay/1 sas".utf8), transcript)
        let v = UInt32(d[0]) | UInt32(d[1]) << 8 | UInt32(d[2]) << 16 | UInt32(d[3]) << 24
        return v % 1_000_000
    }

    public static func sasString(transcript: Data) -> String {
        String(format: "%06u", sasCode(transcript: transcript))
    }

    /// ECDH shared secret: the 32-byte X coordinate.
    public static func ecdh(privateKey: P256.KeyAgreement.PrivateKey, peerPublic x963: Data) throws -> Data {
        let peer = try P256.KeyAgreement.PublicKey(x963Representation: x963)
        let shared = try privateKey.sharedSecretFromKeyAgreement(with: peer)
        return shared.withUnsafeBytes { Data($0) }
    }

    public struct Keys: Sendable, Equatable {
        public let hostToClient: Data
        public let clientToHost: Data
        public let confirm: Data
    }

    /// `okm = HKDF-SHA256(salt=transcript, ikm=Z || psk, info="anydisplay/1 keys", L=96)`
    public static func keySchedule(z: Data, psk: Data, transcript: Data) -> Keys {
        let okm = HKDF<SHA256>.deriveKey(inputKeyMaterial: SymmetricKey(data: z + psk), salt: transcript,
                                         info: Data("anydisplay/1 keys".utf8), outputByteCount: 96)
        let bytes = okm.withUnsafeBytes { Data($0) }
        return Keys(hostToClient: bytes.subdata(in: 0..<32), clientToHost: bytes.subdata(in: 32..<64),
                    confirm: bytes.subdata(in: 64..<96))
    }

    /// `HMAC-SHA256(k_confirm, who || transcript)` with who = "host" or "client".
    public static func confirmMAC(confirmKey: Data, who: String, transcript: Data) -> Data {
        Data(HMAC<SHA256>.authenticationCode(for: Data(who.utf8) + transcript, using: SymmetricKey(data: confirmKey)))
    }

    /// Constant-time comparison.
    public static func equal(_ a: Data, _ b: Data) -> Bool {
        guard a.count == b.count else { return false }
        var diff: UInt8 = 0
        for (x, y) in zip(a, b) { diff |= x ^ y }
        return diff == 0
    }

    public static func randomBytes(_ n: Int) -> Data {
        var g = SystemRandomNumberGenerator()
        return Data((0..<n).map { _ in UInt8.random(in: 0...255, using: &g) })
    }
}

public enum CipherError: Error, Sendable {
    case authenticationFailed
}

/// AES-256-GCM per-direction frame cipher (PROTOCOL.md section 3.1).
/// The nonce is `u64 seq` little-endian followed by four zero bytes; seq counts frames from 0.
public struct FrameCipher: Sendable {
    private let key: SymmetricKey
    public private(set) var seq: UInt64 = 0

    public init(key: Data) {
        self.key = SymmetricKey(data: key)
    }

    private func nonce() -> AES.GCM.Nonce {
        var n = Data(count: 12)
        withUnsafeBytes(of: seq.littleEndian) { n.replaceSubrange(0..<8, with: $0) }
        return try! AES.GCM.Nonce(data: n)
    }

    /// Returns ciphertext || 16-byte tag, authenticating `channel || type`.
    public mutating func seal(_ frame: Frame) -> Frame {
        let box = try! AES.GCM.seal(frame.body, using: key, nonce: nonce(), authenticating: frame.aad)
        seq += 1
        return Frame(channel: frame.channel, type: frame.type, body: box.ciphertext + box.tag)
    }

    public mutating func open(_ frame: Frame) throws -> Frame {
        guard frame.body.count >= 16 else { throw CipherError.authenticationFailed }
        let body = frame.body
        let split = body.endIndex - 16
        do {
            let box = try AES.GCM.SealedBox(nonce: nonce(), ciphertext: body[body.startIndex..<split], tag: body[split...])
            let plain = try AES.GCM.open(box, using: key, authenticating: frame.aad)
            seq += 1
            return Frame(channel: frame.channel, type: frame.type, body: plain)
        } catch {
            throw CipherError.authenticationFailed
        }
    }
}
