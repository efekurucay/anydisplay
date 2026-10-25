import CryptoKit
import XCTest
@testable import AnyDisplayProtocol

/// Minimal client side of the handshake, written from PROTOCOL.md section 4 for driving the host state machine.
struct SimClient {
    let key = P256.KeyAgreement.PrivateKey()
    let nonce = AnyDisplayCrypto.randomBytes(32)
    let deviceID = AnyDisplayCrypto.randomBytes(16)
    var link: UInt8 = 0
    var helloCBody = Data()

    mutating func helloC(commit: Data? = nil) -> Frame {
        let hello = HelloC(deviceID: deviceID, deviceName: "Sim", link: link, authRequired: link == 1 ? 1 : 0,
                           clientPub: key.publicKey.x963Representation,
                           clientCommit: commit ?? AnyDisplayCrypto.sha256(nonce))
        helloCBody = hello.encode()
        return Frame(.control, MessageType.helloC, helloCBody)
    }

    func transcript(helloH: Frame) -> Data {
        AnyDisplayCrypto.transcript(helloC: helloCBody, helloH: helloH.body, reveal: nonce)
    }

    func keys(helloH: Frame, psk: Data) throws -> AnyDisplayCrypto.Keys {
        let h = try HelloH(body: helloH.body)
        let z = try AnyDisplayCrypto.ecdh(privateKey: key, peerPublic: h.hostPub)
        return AnyDisplayCrypto.keySchedule(z: z, psk: psk, transcript: transcript(helloH: helloH))
    }
}

final class HandshakeTests: XCTestCase {
    func makeHost(secret: Data? = nil) -> HostHandshake {
        HostHandshake(hostID: Data(repeating: 1, count: 16), hostName: "Host", hostOS: "macOS",
                      secretForDevice: { _ in secret })
    }

    func sent(_ events: [HostHandshake.Event]) -> [Frame] {
        events.compactMap { if case .send(let f) = $0 { return f } else { return nil } }
    }

    func failure(_ events: [HostHandshake.Event]) -> HandshakeError? {
        for e in events { if case .failed(let err) = e { return err } }
        return nil
    }

    func testMode0Loopback() throws {
        var c = SimClient()
        let hs = makeHost()
        let helloH = try XCTUnwrap(sent(hs.receive(c.helloC())).first)
        XCTAssertEqual(try HelloH(body: helloH.body).authMode, AuthMode.none.rawValue)
        let authH = try XCTUnwrap(sent(hs.receive(Frame(.control, MessageType.reveal, c.nonce))).first)
        let keys = try c.keys(helloH: helloH, psk: Data())
        XCTAssertEqual(authH.body, AnyDisplayCrypto.confirmMAC(confirmKey: keys.confirm, who: "host", transcript: c.transcript(helloH: helloH)))
        let ev = hs.receive(Frame(.control, MessageType.authC,
                                  AnyDisplayCrypto.confirmMAC(confirmKey: keys.confirm, who: "client", transcript: c.transcript(helloH: helloH))))
        XCTAssertEqual(ev.count, 1, "mode 0 does not enable encryption")
        guard case .completed(let r) = ev[0] else { return XCTFail() }
        XCTAssertEqual(r.mode, AuthMode.none)
        XCTAssertNil(r.pairSecret)
    }

    func testMode1RequiredOnNetworkLink() throws {
        var c = SimClient()
        c.link = 1
        let hs = makeHost()
        let helloH = try XCTUnwrap(sent(hs.receive(c.helloC())).first)
        XCTAssertEqual(try HelloH(body: helloH.body).authMode, AuthMode.sas.rawValue)
        let ev = hs.receive(Frame(.control, MessageType.reveal, c.nonce))
        guard case .showSAS(let code) = ev.first else { return XCTFail() }
        XCTAssertEqual(code, AnyDisplayCrypto.sasString(transcript: c.transcript(helloH: helloH)))
        // The user confirms before the peer does: SAS_OK goes out, AUTH_H waits for the peer's SAS_OK.
        XCTAssertEqual(sent(hs.userDecision(confirmed: true)).map(\.type), [MessageType.sasOK])
        XCTAssertEqual(sent(hs.receive(Frame(.control, MessageType.sasOK))).map(\.type), [MessageType.authH])
    }

    func testMode1UserRejects() throws {
        var c = SimClient()
        c.link = 1
        let hs = makeHost()
        _ = hs.receive(c.helloC())
        _ = hs.receive(Frame(.control, MessageType.reveal, c.nonce))
        let ev = hs.userDecision(confirmed: false)
        XCTAssertEqual(sent(ev), [Frame(.control, MessageType.authFail, Data([AuthFailReason.userRejected.rawValue]))])
        XCTAssertEqual(failure(ev), .localFailed(.userRejected))
    }

    func testMode2UsesStoredSecret() throws {
        var c = SimClient()
        c.link = 1
        let secret = AnyDisplayCrypto.randomBytes(32)
        let hs = makeHost(secret: secret)
        let helloH = try XCTUnwrap(sent(hs.receive(c.helloC())).first)
        XCTAssertEqual(try HelloH(body: helloH.body).authMode, AuthMode.paired.rawValue)
        let authH = try XCTUnwrap(sent(hs.receive(Frame(.control, MessageType.reveal, c.nonce))).first)
        let t = c.transcript(helloH: helloH)
        let keys = try c.keys(helloH: helloH, psk: secret)
        XCTAssertEqual(authH.body, AnyDisplayCrypto.confirmMAC(confirmKey: keys.confirm, who: "host", transcript: t))
        let ev = hs.receive(Frame(.control, MessageType.authC, AnyDisplayCrypto.confirmMAC(confirmKey: keys.confirm, who: "client", transcript: t)))
        guard case .enableEncryption(let s, let r) = ev[0], case .completed(let res) = ev[1] else { return XCTFail() }
        XCTAssertEqual(s, keys.hostToClient)
        XCTAssertEqual(r, keys.clientToHost)
        XCTAssertEqual(res.mode, .paired)
    }

    func testMode2WrongSecretFailsMAC() throws {
        var c = SimClient()
        let hs = makeHost(secret: AnyDisplayCrypto.randomBytes(32))
        let helloH = try XCTUnwrap(sent(hs.receive(c.helloC())).first)
        _ = hs.receive(Frame(.control, MessageType.reveal, c.nonce))
        let keys = try c.keys(helloH: helloH, psk: AnyDisplayCrypto.randomBytes(32))
        let ev = hs.receive(Frame(.control, MessageType.authC,
                                  AnyDisplayCrypto.confirmMAC(confirmKey: keys.confirm, who: "client", transcript: c.transcript(helloH: helloH))))
        XCTAssertEqual(sent(ev), [Frame(.control, MessageType.authFail, Data([AuthFailReason.badMAC.rawValue]))])
        XCTAssertEqual(failure(ev), .localFailed(.badMAC))
    }

    func testUnknownHostFromClient() {
        var c = SimClient()
        let hs = makeHost(secret: AnyDisplayCrypto.randomBytes(32))
        _ = hs.receive(c.helloC())
        let ev = hs.receive(Frame(.control, MessageType.authFail, Data([AuthFailReason.unknownHost.rawValue])))
        XCTAssertEqual(failure(ev), .peerFailed(AuthFailReason.unknownHost.rawValue))
        XCTAssertTrue(sent(ev).isEmpty)
    }

    func testBadCommit() {
        var c = SimClient()
        let hs = makeHost()
        _ = hs.receive(c.helloC(commit: Data(repeating: 0, count: 32)))
        let ev = hs.receive(Frame(.control, MessageType.reveal, c.nonce))
        XCTAssertEqual(sent(ev), [Frame(.control, MessageType.authFail, Data([AuthFailReason.badCommit.rawValue]))])
    }

    func testBadVersion() {
        var c = SimClient()
        var hello = c.helloC()
        hello.body.replaceSubrange(4..<6, with: [2, 0])
        let ev = makeHost().receive(hello)
        XCTAssertEqual(sent(ev), [Frame(.control, MessageType.authFail, Data([AuthFailReason.badVersion.rawValue]))])
        XCTAssertEqual(failure(ev), .localFailed(.badVersion))
    }

    func testTrailingBytesInHelloCAreKeptInTranscript() throws {
        var c = SimClient()
        var hello = c.helloC()
        hello.body.append(contentsOf: [9, 9])
        c.helloCBody = hello.body
        let hs = makeHost()
        let helloH = try XCTUnwrap(sent(hs.receive(hello)).first)
        let authH = try XCTUnwrap(sent(hs.receive(Frame(.control, MessageType.reveal, c.nonce))).first)
        let keys = try c.keys(helloH: helloH, psk: Data())
        XCTAssertEqual(authH.body, AnyDisplayCrypto.confirmMAC(confirmKey: keys.confirm, who: "host", transcript: c.transcript(helloH: helloH)))
    }
}

final class FramingTests: XCTestCase {
    func testParserAcrossChunks() throws {
        let frames = [Frame(.control, 0x13, Data([1, 2, 3])), Frame(.video, 0x20, Data(repeating: 7, count: 1000)), Frame(.input, 0x16)]
        let wire = frames.map(\.wireBytes).reduce(Data(), +)
        var p = FrameParser()
        var out: [Frame] = []
        for i in stride(from: 0, to: wire.count, by: 7) {
            p.append(wire.subdata(in: i..<min(i + 7, wire.count)))
            while let f = try p.next() { out.append(f) }
        }
        XCTAssertEqual(out, frames)
        XCTAssertEqual(frames[0].wireBytes, Data([5, 0, 0, 0, 0, 0x13, 1, 2, 3]))
    }

    func testParserRejectsBadLength() {
        var p = FrameParser()
        p.append(Data([1, 0, 0, 0, 0, 0]))
        XCTAssertThrowsError(try p.next())
        var q = FrameParser()
        q.append(Data([0x01, 0x00, 0x00, 0x02, 0, 0]))  // 32 MiB + 1
        XCTAssertThrowsError(try q.next())
    }

    func testCipherSequenceAndTamper() throws {
        let key = AnyDisplayCrypto.randomBytes(32)
        var tx = FrameCipher(key: key)
        var rx = FrameCipher(key: key)
        for i in 0..<3 {
            let f = Frame(.input, MessageType.text, Text(text: "hi \(i)").encode())
            let sealed = tx.seal(f)
            XCTAssertEqual(sealed.body.count, f.body.count + 16)
            XCTAssertEqual(try rx.open(sealed), f)
        }
        var bad = tx.seal(Frame(.control, MessageType.ping, Data([1])))
        bad.type = MessageType.pong  // AAD covers channel||type
        XCTAssertThrowsError(try rx.open(bad))
    }

    func testInputMessagesRoundTrip() throws {
        let pen = Pen(samples: [PenSample(action: 0, tool: 1, buttons: 1, x: 100, y: 65535, pressure: 30000,
                                          tiltX: -9000, tiltY: 4500, twist: 1234, timeMs: 99)])
        XCTAssertEqual(try Pen(body: pen.encode()), pen)
        XCTAssertEqual(pen.encode().count, 1 + 19)
        let touch = Touch(contacts: [TouchContact(id: 1, action: 0, x: 1, y: 2, pressure: 3, size: 4)])
        XCTAssertEqual(try Touch(body: touch.encode()), touch)
        let cfg = Config(streamID: 1, width: 1920, height: 1080, refreshHz: 60, codec: 1, bitrateKbps: 8000, flags: 1,
                         logicalW: 960, logicalH: 540, hostCaps: 6)
        XCTAssertEqual(cfg.encode().count, 24)
        XCTAssertEqual(try Config(body: cfg.encode() + Data([0xFF])), cfg, "trailing bytes are ignored")
    }
}

final class PairingStoreTests: XCTestCase {
    func testPersistence() throws {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("anydisplay-test-\(UUID().uuidString)").appendingPathComponent("host.json")
        defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
        let a = PairingStore(url: url)
        XCTAssertEqual(a.hostID.count, 16)
        let dev = Data(repeating: 0xAB, count: 16)
        let secret = Data(repeating: 0xCD, count: 32)
        a.pair(deviceID: dev, name: "Tab", secret: secret)

        let b = PairingStore(url: url)
        XCTAssertEqual(b.hostID, a.hostID)
        XCTAssertEqual(b.secret(for: dev), secret)
        XCTAssertEqual(b.devices[dev.hex]?.name, "Tab")
        let attrs = try FileManager.default.attributesOfItem(atPath: url.path)
        XCTAssertEqual((attrs[.posixPermissions] as? NSNumber)?.intValue, 0o600)

        b.forget(deviceID: dev)
        XCTAssertNil(PairingStore(url: url).secret(for: dev))
    }
}
