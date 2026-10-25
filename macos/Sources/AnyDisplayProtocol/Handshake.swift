import CryptoKit
import Foundation

public enum HandshakeError: Error, Equatable, Sendable, CustomStringConvertible {
    /// The client sent `AUTH_FAIL` with this reason byte.
    case peerFailed(UInt8)
    /// We sent `AUTH_FAIL` with this reason.
    case localFailed(AuthFailReason)
    case protocolViolation(String)
    case timeout

    public var description: String {
        switch self {
        case .peerFailed(let r): return "client sent AUTH_FAIL(\(AuthFailReason(rawValue: r).map { "\($0)" } ?? "\(r)"))"
        case .localFailed(let r): return "handshake failed: \(r)"
        case .protocolViolation(let s): return "protocol violation: \(s)"
        case .timeout: return "handshake timed out"
        }
    }
}

public struct HandshakeResult: Sendable {
    public let deviceID: Data
    public let deviceName: String
    public let link: UInt8
    public let mode: AuthMode
    public let transcript: Data
    public let keys: AnyDisplayCrypto.Keys
    /// Set in mode 1: the `pair_secret` the client sent in `PAIR`, to be persisted for `deviceID`.
    public let pairSecret: Data?
}

/// Host side of PROTOCOL.md section 4, driven by received frames and the local user's SAS decision.
/// Pure: it never touches the network; the caller applies the returned events in order.
public final class HostHandshake {
    public enum Event: Sendable {
        case send(Frame)
        /// Mode 1: show this 6-digit code and call `userDecision(confirmed:)`.
        case showSAS(String)
        /// Encrypt every frame from now on in both directions (sent after `AUTH_C` was verified).
        case enableEncryption(send: Data, receive: Data)
        case completed(HandshakeResult)
        /// Terminal: close the connection after flushing any `.send` that preceded it.
        case failed(HandshakeError)
    }

    public enum State: Equatable, Sendable {
        case awaitingHelloC, awaitingReveal, awaitingSAS, awaitingAuthC, awaitingPair, done, failed
    }

    public private(set) var state: State = .awaitingHelloC

    private let hostID: Data
    private let hostName: String
    private let hostOS: String
    private let secretForDevice: (Data) -> Data?
    private let privateKey: P256.KeyAgreement.PrivateKey
    private let hostNonce: Data

    private var helloCBody = Data()
    private var helloC: HelloC?
    private var helloHBody = Data()
    private var mode: AuthMode = .none
    private var psk = Data()
    private var transcript = Data()
    private var keys: AnyDisplayCrypto.Keys?
    private var userConfirmed = false
    private var peerConfirmed = false

    /// - Parameters:
    ///   - secretForDevice: stored `pair_secret` for a `device_id`, if paired.
    ///   - privateKey, hostNonce: injectable for deterministic tests; fresh per connection otherwise.
    public init(hostID: Data, hostName: String, hostOS: String, secretForDevice: @escaping (Data) -> Data?,
                privateKey: P256.KeyAgreement.PrivateKey = .init(), hostNonce: Data = AnyDisplayCrypto.randomBytes(32)) {
        self.hostID = hostID
        self.hostName = hostName
        self.hostOS = hostOS
        self.secretForDevice = secretForDevice
        self.privateKey = privateKey
        self.hostNonce = hostNonce
    }

    /// PROTOCOL.md 4.3: 10 s per step, 120 s while a user has to compare the code.
    public var stepTimeout: TimeInterval { state == .awaitingSAS ? 120 : 10 }

    /// The authentication mode picked after `HELLO_C`.
    public var authMode: AuthMode { mode }

    /// The `HELLO_C` the client sent, once received.
    public var clientHello: HelloC? { helloC }

    public func receive(_ frame: Frame) -> [Event] {
        guard state != .done, state != .failed else { return [] }
        guard frame.channel == Channel.control.rawValue else {
            return fail(.protocolViolation("channel \(frame.channel) frame during handshake"))
        }
        if frame.type == MessageType.authFail {
            let reason = frame.body.first ?? 0
            return fail(.peerFailed(reason))
        }
        switch state {
        case .awaitingHelloC: return onHelloC(frame)
        case .awaitingReveal: return onReveal(frame)
        case .awaitingSAS: return onSASFrame(frame)
        case .awaitingAuthC: return onAuthC(frame)
        case .awaitingPair: return onPair(frame)
        case .done, .failed: return []
        }
    }

    /// The local user's answer to the SAS comparison (mode 1).
    public func userDecision(confirmed: Bool) -> [Event] {
        guard state == .awaitingSAS, !userConfirmed else { return [] }
        guard confirmed else { return failSending(.userRejected) }
        userConfirmed = true
        var events: [Event] = [.send(Frame(.control, MessageType.sasOK))]
        if peerConfirmed { events += sendAuthH() }
        return events
    }

    /// Call when `stepTimeout` elapses without progress.
    public func timedOut() -> [Event] {
        guard state != .done, state != .failed else { return [] }
        return fail(.timeout)
    }

    // MARK: - Steps

    private func onHelloC(_ frame: Frame) -> [Event] {
        guard frame.type == MessageType.helloC else { return fail(.protocolViolation("expected HELLO_C")) }
        let hello: HelloC
        do { hello = try HelloC(body: frame.body) } catch {
            // A body too short to hold the v1 fields cannot be a v1 client.
            return failSending(.badVersion)
        }
        guard hello.magic == anyDisplayMagic, hello.major == anyDisplayMajor else { return failSending(.badVersion) }
        guard (try? P256.KeyAgreement.PublicKey(x963Representation: hello.clientPub)) != nil else {
            return fail(.protocolViolation("invalid client_pub"))
        }
        helloC = hello
        helloCBody = frame.body

        if let secret = secretForDevice(hello.deviceID) {
            mode = .paired
            psk = secret
        } else {
            mode = hello.authRequired == 0 ? .none : .sas
            psk = Data()
        }
        let helloH = HelloH(hostID: hostID, hostName: hostName, hostOS: hostOS, authMode: mode.rawValue,
                            hostPub: privateKey.publicKey.x963Representation, hostNonce: hostNonce)
        helloHBody = helloH.encode()
        state = .awaitingReveal
        return [.send(Frame(.control, MessageType.helloH, helloHBody))]
    }

    private func onReveal(_ frame: Frame) -> [Event] {
        guard frame.type == MessageType.reveal, frame.body.count >= 32, let hello = helloC else {
            return fail(.protocolViolation("expected REVEAL"))
        }
        let clientNonce = frame.body.prefix(32)
        guard AnyDisplayCrypto.sha256(Data(clientNonce)) == hello.clientCommit else { return failSending(.badCommit) }
        transcript = AnyDisplayCrypto.transcript(helloC: helloCBody, helloH: helloHBody, reveal: frame.body)
        do {
            let z = try AnyDisplayCrypto.ecdh(privateKey: privateKey, peerPublic: hello.clientPub)
            keys = AnyDisplayCrypto.keySchedule(z: z, psk: psk, transcript: transcript)
        } catch {
            return fail(.protocolViolation("ECDH failed"))
        }
        if mode == .sas {
            state = .awaitingSAS
            return [.showSAS(AnyDisplayCrypto.sasString(transcript: transcript))]
        }
        return sendAuthH()
    }

    private func onSASFrame(_ frame: Frame) -> [Event] {
        guard frame.type == MessageType.sasOK, !peerConfirmed else { return fail(.protocolViolation("expected SAS_OK")) }
        peerConfirmed = true
        return userConfirmed ? sendAuthH() : []
    }

    private func sendAuthH() -> [Event] {
        guard let keys else { return fail(.protocolViolation("no keys")) }
        state = .awaitingAuthC
        let mac = AnyDisplayCrypto.confirmMAC(confirmKey: keys.confirm, who: "host", transcript: transcript)
        return [.send(Frame(.control, MessageType.authH, mac))]
    }

    private func onAuthC(_ frame: Frame) -> [Event] {
        guard frame.type == MessageType.authC, let keys else { return fail(.protocolViolation("expected AUTH_C")) }
        let expected = AnyDisplayCrypto.confirmMAC(confirmKey: keys.confirm, who: "client", transcript: transcript)
        guard AnyDisplayCrypto.equal(Data(frame.body.prefix(32)), expected) else { return failSending(.badMAC) }
        switch mode {
        case .none:
            return complete(pairSecret: nil)
        case .paired:
            return [.enableEncryption(send: keys.hostToClient, receive: keys.clientToHost)] + complete(pairSecret: nil)
        case .sas:
            state = .awaitingPair
            return [.enableEncryption(send: keys.hostToClient, receive: keys.clientToHost)]
        }
    }

    private func onPair(_ frame: Frame) -> [Event] {
        guard frame.type == MessageType.pair, frame.body.count >= 32 else { return fail(.protocolViolation("expected PAIR")) }
        return complete(pairSecret: Data(frame.body.prefix(32)))
    }

    private func complete(pairSecret: Data?) -> [Event] {
        guard let hello = helloC, let keys else { return fail(.protocolViolation("incomplete handshake")) }
        state = .done
        return [.completed(HandshakeResult(deviceID: hello.deviceID, deviceName: hello.deviceName, link: hello.link,
                                           mode: mode, transcript: transcript, keys: keys, pairSecret: pairSecret))]
    }

    private func failSending(_ reason: AuthFailReason) -> [Event] {
        state = .failed
        return [.send(Frame(.control, MessageType.authFail, Data([reason.rawValue]))), .failed(.localFailed(reason))]
    }

    private func fail(_ error: HandshakeError) -> [Event] {
        state = .failed
        return [.failed(error)]
    }
}
