import CryptoKit
import Foundation

/// Encrypted network pairing for native clients. The caller applies events in wire order.
public final class ClientHandshake {
    public enum Event {
        case send(Frame)
        case showCode(String, host: HelloH)
        case enableEncryption(UseThatScreenCrypto.Keys)
        case storePair(host: HelloH, secret: Data)
        case completed(HelloH)
        case failed(String)
    }

    private enum State { case idle, hello, comparison, authentication, terminal }
    private var state = State.idle
    private let secretForHost: (Data) -> Data?
    private let privateKey = P256.KeyAgreement.PrivateKey()
    private let nonce = UseThatScreenCrypto.randomBytes(32)
    private let deviceID: Data
    private let deviceName: String
    private var hello = Data()
    private var host: HelloH?
    private var keys: UseThatScreenCrypto.Keys?
    private var transcript = Data()
    private var hostConfirmed = false
    private var clientConfirmed = false

    public init(deviceID: Data, deviceName: String, secretForHost: @escaping (Data) -> Data?) {
        self.deviceID = deviceID
        self.deviceName = deviceName
        self.secretForHost = secretForHost
    }

    public var stepTimeout: TimeInterval { state == .comparison ? 120 : 10 }

    public func start() -> [Event] {
        guard state == .idle, deviceID.count == 16 else { return [] }
        let message = HelloC(deviceID: deviceID, deviceName: deviceName, link: Link.network.rawValue,
                             authRequired: 1, clientPub: privateKey.publicKey.x963Representation,
                             clientCommit: UseThatScreenCrypto.sha256(nonce))
        hello = message.encode()
        state = .hello
        return [.send(Frame(.control, MessageType.helloC, hello))]
    }

    public func receive(_ frame: Frame) -> [Event] {
        guard state != .idle, state != .terminal else { return [] }
        guard frame.channel == Channel.control.rawValue else {
            return fail(.badVersion, "Unexpected channel during pairing")
        }
        if frame.type == MessageType.authFail {
            state = .terminal
            let reason = frame.body.first.flatMap(AuthFailReason.init(rawValue:))
            return [.failed(reason == .busy ? "The computer is busy" : "The computer rejected pairing")]
        }
        switch state {
        case .hello where frame.type == MessageType.helloH:
            return receiveHostHello(frame.body)
        case .comparison where frame.type == MessageType.sasOK && frame.body.isEmpty && !hostConfirmed:
            hostConfirmed = true
            if clientConfirmed { state = .authentication }
            return []
        case .authentication where frame.type == MessageType.authH:
            return authenticate(frame.body)
        default:
            return fail(.badVersion, "Unexpected message during pairing")
        }
    }

    public func confirmCode(_ accepted: Bool) -> [Event] {
        guard state == .comparison, !clientConfirmed else { return [] }
        guard accepted else { return fail(.userRejected, "Pairing cancelled") }
        clientConfirmed = true
        if hostConfirmed { state = .authentication }
        return [.send(Frame(.control, MessageType.sasOK))]
    }

    private func receiveHostHello(_ body: Data) -> [Event] {
        guard let host = try? HelloH(body: body),
              host.magic == useThatScreenMagic, host.major == useThatScreenMajor,
              let mode = AuthMode(rawValue: host.authMode) else {
            return fail(.badVersion, "The computer speaks a different protocol version")
        }
        guard mode != .none else { return fail(.authRequired, "Wi-Fi requires encrypted pairing") }
        let secret: Data
        if mode == .paired {
            guard let stored = secretForHost(host.hostID), stored.count == 32 else {
                return fail(.unknownHost, "Forget this iPad on the computer, then pair again")
            }
            secret = stored
        } else {
            secret = Data()
        }
        guard let agreement = try? UseThatScreenCrypto.ecdh(privateKey: privateKey, peerPublic: host.hostPub) else {
            return fail(.badMAC, "The computer supplied an invalid key")
        }
        self.host = host
        transcript = UseThatScreenCrypto.transcript(helloC: hello, helloH: body, reveal: nonce)
        keys = UseThatScreenCrypto.keySchedule(z: agreement, psk: secret, transcript: transcript)
        let reveal = Event.send(Frame(.control, MessageType.reveal, nonce))
        if mode == .sas {
            state = .comparison
            return [reveal, .showCode(UseThatScreenCrypto.sasString(transcript: transcript), host: host)]
        }
        state = .authentication
        return [reveal]
    }

    private func authenticate(_ body: Data) -> [Event] {
        guard let keys, let host else { return fail(.badMAC, "Missing pairing keys") }
        let expected = UseThatScreenCrypto.confirmMAC(confirmKey: keys.confirm, who: "host", transcript: transcript)
        guard UseThatScreenCrypto.equal(body, expected) else {
            return fail(.badMAC, "The computer could not verify this pairing")
        }
        let confirmation = UseThatScreenCrypto.confirmMAC(confirmKey: keys.confirm, who: "client", transcript: transcript)
        var events: [Event] = [.send(Frame(.control, MessageType.authC, confirmation)), .enableEncryption(keys)]
        if host.authMode == AuthMode.sas.rawValue {
            let secret = UseThatScreenCrypto.randomBytes(32)
            events += [.storePair(host: host, secret: secret), .send(Frame(.control, MessageType.pair, secret))]
        }
        state = .terminal
        events.append(.completed(host))
        return events
    }

    private func fail(_ reason: AuthFailReason, _ message: String) -> [Event] {
        state = .terminal
        return [.send(Frame(.control, MessageType.authFail, Data([reason.rawValue]))), .failed(message)]
    }
}
