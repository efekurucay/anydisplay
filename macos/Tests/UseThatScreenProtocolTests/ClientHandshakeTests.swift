import XCTest
@testable import UseThatScreenProtocol

final class ClientHandshakeTests: XCTestCase {
    private let deviceID = Data(repeating: 1, count: 16)
    private let hostID = Data(repeating: 2, count: 16)

    private func client(secret: Data? = nil) -> ClientHandshake {
        ClientHandshake(deviceID: deviceID, deviceName: "iPad", secretForHost: { _ in secret })
    }

    private func host(secret: Data? = nil) -> HostHandshake {
        HostHandshake(hostID: hostID, hostName: "Mac", hostOS: "macOS", secretForDevice: { _ in secret })
    }

    private func clientFrame(_ events: [ClientHandshake.Event], _ type: UInt8) throws -> Frame {
        try XCTUnwrap(events.compactMap {
            if case .send(let frame) = $0, frame.type == type { return frame }
            return nil
        }.first)
    }

    private func hostFrame(_ events: [HostHandshake.Event], _ type: UInt8) throws -> Frame {
        try XCTUnwrap(events.compactMap {
            if case .send(let frame) = $0, frame.type == type { return frame }
            return nil
        }.first)
    }

    func testNetworkPairingAndEncryptedReconnect() throws {
        let client = client()
        let host = host()
        let helloC = try clientFrame(client.start(), MessageType.helloC)
        let helloH = try hostFrame(host.receive(helloC), MessageType.helloH)
        let events = client.receive(helloH)
        let reveal = try clientFrame(events, MessageType.reveal)
        guard case .showCode(let clientCode, _) = events.last,
              case .showSAS(let hostCode) = host.receive(reveal).first else { return XCTFail("Missing SAS") }
        XCTAssertEqual(clientCode, hostCode)
        let hostOK = try hostFrame(host.userDecision(confirmed: true), MessageType.sasOK)
        XCTAssertTrue(client.receive(hostOK).isEmpty)
        let clientOK = try clientFrame(client.confirmCode(true), MessageType.sasOK)
        let authH = try hostFrame(host.receive(clientOK), MessageType.authH)
        let authenticated = client.receive(authH)
        let authC = try clientFrame(authenticated, MessageType.authC)
        guard authenticated.count == 5, case .enableEncryption(let keys) = authenticated[1],
              case .storePair(_, let secret) = authenticated[2], case .completed = authenticated[4] else {
            return XCTFail("Expected plaintext confirmation before enabling encryption and saving the pair")
        }
        guard case .enableEncryption(let send, let receive) = host.receive(authC).first else {
            return XCTFail("Host did not enable encryption")
        }
        XCTAssertEqual(keys.hostToClient, send)
        var clientCipher = FrameCipher(key: keys.clientToHost)
        var hostCipher = FrameCipher(key: receive)
        let pair = try clientFrame(authenticated, MessageType.pair)
        guard case .completed(let paired) = host.receive(try hostCipher.open(clientCipher.seal(pair))).first else {
            return XCTFail("Host did not accept the encrypted pair")
        }
        XCTAssertEqual(paired.pairSecret, secret)

        let returningClient = self.client(secret: secret)
        let returningHost = self.host(secret: secret)
        let returningHello = try hostFrame(returningHost.receive(clientFrame(returningClient.start(), MessageType.helloC)),
                                           MessageType.helloH)
        let returningReveal = try clientFrame(returningClient.receive(returningHello), MessageType.reveal)
        let returningAuth = try hostFrame(returningHost.receive(returningReveal), MessageType.authH)
        let reconnected = returningClient.receive(returningAuth)
        XCTAssertEqual(reconnected.count, 3)
        guard case .completed = reconnected.last else { return XCTFail("Expected an encrypted reconnect without SAS") }
        let result = returningHost.receive(try clientFrame(reconnected, MessageType.authC))
        guard case .completed(let authenticatedHost) = result.last else { return XCTFail("Host did not verify the pair") }
        XCTAssertEqual(authenticatedHost.mode, .paired)
    }

    func testWiFiClientRejectsUnauthenticatedHost() throws {
        let client = client()
        _ = client.start()
        let hello = HelloH(hostID: hostID, hostName: "Mac", hostOS: "macOS", authMode: AuthMode.none.rawValue,
                           hostPub: Data(repeating: 0, count: 65), hostNonce: Data(repeating: 0, count: 32))
        let response = try clientFrame(client.receive(hello.frame), MessageType.authFail)
        XCTAssertEqual(response.body, Data([AuthFailReason.authRequired.rawValue]))
    }
}
