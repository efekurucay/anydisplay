import Foundation
import Network
import UIKit
import UseThatScreenProtocol

@MainActor
final class DisplaySession: ObservableObject {
    struct Comparison {
        let code: String
        let hostName: String
    }

    enum Mode: String, CaseIterable, Identifiable {
        case extend = "Extend", mirror = "Mirror"
        var id: Self { self }
    }

    @Published var mode: Mode = .extend
    @Published private(set) var status = "Starting local discovery…"
    @Published private(set) var comparison: Comparison?
    @Published private(set) var config: Config?
    @Published private(set) var receivedVideo = false
    @Published private(set) var hostName = ""
    @Published private(set) var notice: String?
    let renderer = StreamRenderer()
    let deviceName: String

    private let deviceID: Data
    private var listener: NWListener?
    private var connection: NWConnection?
    private var handshake: ClientHandshake?
    private var parser = FrameParser()
    private var sendCipher: FrameCipher?
    private var receiveCipher: FrameCipher?
    private var timeout: DispatchWorkItem?
    private var surfaceSize = CGSize.zero
    private var lastKeyframeRequest = -Double.infinity
    private var keyframeRequested = false

    init() {
        let defaults = UserDefaults.standard
        if let saved = defaults.data(forKey: "deviceID"), saved.count == 16 {
            deviceID = saved
        } else {
            deviceID = UseThatScreenCrypto.randomBytes(16)
            defaults.set(deviceID, forKey: "deviceID")
        }
        deviceName = "iPad " + deviceID.suffix(2).map { String(format: "%02X", $0) }.joined()
    }

    func start(size: CGSize) {
        if size.width >= 2, size.height >= 2 { surfaceSize = size }
        guard listener == nil else { return }
        do {
            let params = NWParameters.tcp
            (params.defaultProtocolStack.transportProtocol as? NWProtocolTCP.Options)?.noDelay = true
            let listener = try NWListener(using: params, on: NWEndpoint.Port(rawValue: useThatScreenPort)!)
            let id = deviceID.map { String(format: "%02x", $0) }.joined()
            listener.service = NWListener.Service(name: deviceName, type: useThatScreenBonjourType,
                                                  txtRecord: NWTXTRecord(["id": id, "v": "1"]))
            // Network invokes these callbacks on the queue passed to start(queue:).
            listener.stateUpdateHandler = { [weak self, weak listener] state in
                MainActor.assumeIsolated {
                    guard let self, let listener, self.listener === listener else { return }
                    switch state {
                    case .ready:
                        if self.connection == nil { self.status = "Waiting for a computer" }
                    case .waiting(let error):
                        self.status = "Local network unavailable"
                        self.notice = error.localizedDescription
                    case .failed(let error):
                        listener.cancel()
                        self.listener = nil
                        self.status = "Could not start local discovery"
                        self.notice = error.localizedDescription
                    default: break
                    }
                }
            }
            listener.newConnectionHandler = { [weak self, weak listener] connection in
                MainActor.assumeIsolated {
                    guard let self, let listener, self.listener === listener else { connection.cancel(); return }
                    self.accept(connection)
                }
            }
            self.listener = listener
            notice = nil
            listener.start(queue: .main)
        } catch {
            status = "Could not start local discovery"
            notice = error.localizedDescription
        }
    }

    func stop() {
        listener?.cancel()
        listener = nil
        disconnect()
        status = "Local discovery paused"
    }

    func disconnect() {
        if sendCipher != nil, let connection {
            let frame = Bye(reason: ByeReason.user.rawValue).frame
            let bytes = (sendCipher?.seal(frame) ?? frame).wireBytes
            connection.send(content: bytes, completion: .contentProcessed { _ in connection.cancel() })
            self.connection = nil
        }
        reset()
    }

    func confirmCode(_ accepted: Bool) {
        comparison = nil
        guard let handshake else { return }
        apply(handshake.confirmCode(accepted))
    }

    private func accept(_ connection: NWConnection) {
        guard self.connection == nil else { connection.cancel(); return }
        self.connection = connection
        notice = nil
        status = "Connecting…"
        let handshake = ClientHandshake(deviceID: deviceID, deviceName: deviceName, secretForHost: PairingKeys.secret)
        self.handshake = handshake
        connection.stateUpdateHandler = { [weak self, weak connection] state in
            MainActor.assumeIsolated {
                guard let self, let connection, self.connection === connection else { return }
                switch state {
                case .ready:
                    if let handshake = self.handshake { self.apply(handshake.start()) }
                    self.receive(on: connection)
                case .failed(let error):
                    self.reset(message: error.localizedDescription)
                default: break
                }
            }
        }
        armTimeout(seconds: handshake.stepTimeout)
        connection.start(queue: .main)
    }

    private func receive(on connection: NWConnection) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self, weak connection] data, _, done, error in
            MainActor.assumeIsolated {
                guard let self, let connection, self.connection === connection else { return }
                self.receive(data, done: done, error: error, on: connection)
            }
        }
    }

    private func receive(_ data: Data?, done: Bool, error: NWError?, on connection: NWConnection) {
        do {
            if let data { parser.append(data) }
            while self.connection === connection, let packet = try parser.next() {
                let frame = try receiveCipher?.open(packet) ?? packet
                if let handshake {
                    apply(handshake.receive(frame))
                } else {
                    try handle(frame)
                    if self.connection === connection { armTimeout(seconds: 8) }
                }
            }
        } catch {
            reset(message: "The stream could not be read: \(error.localizedDescription)")
            return
        }
        guard self.connection === connection else { return }
        if done || error != nil {
            reset(message: error?.localizedDescription ?? "The computer disconnected")
            return
        }
        receive(on: connection)
    }

    private func apply(_ events: [ClientHandshake.Event]) {
        for event in events {
            switch event {
            case .send(let frame): send(frame)
            case .showCode(let code, let host):
                comparison = Comparison(code: code, hostName: host.hostName)
                status = "Compare the pairing code"
            case .enableEncryption(let keys):
                sendCipher = FrameCipher(key: keys.clientToHost)
                receiveCipher = FrameCipher(key: keys.hostToClient)
            case .storePair(let host, let secret):
                if !PairingKeys.save(secret, for: host.hostID) {
                    reset(message: "Could not save pairing in the iPad Keychain")
                    return
                }
            case .completed(let host):
                handshake = nil
                comparison = nil
                hostName = host.hostName
                status = "Waiting for video…"
                sendConfiguration()
            case .failed(let message):
                reset(message: message)
                return
            }
        }
        if connection != nil { armTimeout(seconds: handshake?.stepTimeout ?? 8) }
    }

    private func sendConfiguration() {
        let size = surfaceSize.width >= 2 && surfaceSize.height >= 2 ? surfaceSize : CGSize(width: 1920, height: 1080)
        let w = UInt16(min(size.width, 4096))
        let h = UInt16(min(size.height, 4096))
        send(DeviceInfo(screenW: w, screenH: h, xdpi: 0, ydpi: 0, maxRefresh: 60, codecs: 1,
                        maxDecodeW: 4096, maxDecodeH: 4096, maxTouches: 0, penCaps: 0, audio: 0,
                        model: deviceName, osVersion: "iPadOS \(UIDevice.current.systemVersion)").frame)
        let flags: StreamFlags = mode == .mirror ? [.mirror] : []
        send(ConfigReq(width: w, height: h, refreshHz: 60, codec: Codec.h264.rawValue,
                       bitrateKbps: 0, quality: 65, flags: flags.rawValue).frame)
    }

    private func handle(_ frame: Frame) throws {
        switch (Channel(rawValue: frame.channel), frame.type) {
        case (.control, MessageType.config):
            let config = try Config(body: frame.body)
            guard config.codec == Codec.h264.rawValue, config.width > 0, config.height > 0,
                  config.width <= 4096, config.height <= 4096 else {
                reset(message: "The computer offered an unsupported stream format")
                return
            }
            renderer.reset()
            self.config = config
            receivedVideo = false
            keyframeRequested = false
            lastKeyframeRequest = -.infinity
        case (.control, MessageType.ping):
            let ping = try Ping(body: frame.body)
            send(Pong(seq: ping.seq, echoedTimeUs: ping.senderTimeUs).frame)
        case (.control, MessageType.notice):
            notice = try Notice(body: frame.body).message
        case (.control, MessageType.bye):
            reset(message: notice ?? "The computer disconnected")
        case (.video, MessageType.videoFrame):
            let video = try VideoFrame(body: frame.body)
            guard let config, video.streamID == config.streamID else { return }
            let queued = renderer.enqueue(video)
            send(FrameAck(streamID: video.streamID, frameID: video.frameID, decodeUs: 0).frame)
            if queued {
                if !receivedVideo { receivedVideo = true }
                keyframeRequested = false
            } else if !keyframeRequested || ProcessInfo.processInfo.systemUptime - lastKeyframeRequest >= 1 {
                send(Frame(.control, MessageType.keyframeReq))
                keyframeRequested = true
                lastKeyframeRequest = ProcessInfo.processInfo.systemUptime
            }
        default:
            break
        }
    }

    private func armTimeout(seconds: TimeInterval) {
        timeout?.cancel()
        let task = DispatchWorkItem { [weak self] in self?.reset(message: "The computer stopped responding") }
        timeout = task
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: task)
    }

    private func send(_ frame: Frame) {
        guard let connection else { return }
        let bytes = (sendCipher?.seal(frame) ?? frame).wireBytes
        connection.send(content: bytes, completion: .contentProcessed { [weak self, weak connection] error in
            MainActor.assumeIsolated {
                guard let self, let connection, self.connection === connection, let error else { return }
                self.reset(message: error.localizedDescription)
            }
        })
    }

    private func reset(message: String? = nil) {
        timeout?.cancel()
        timeout = nil
        connection?.cancel()
        connection = nil
        handshake = nil
        parser = FrameParser()
        sendCipher = nil
        receiveCipher = nil
        comparison = nil
        config = nil
        receivedVideo = false
        hostName = ""
        notice = message
        status = "Waiting for a computer"
        renderer.reset()
    }
}
