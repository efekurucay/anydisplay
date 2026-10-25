import ApplicationServices
import CoreGraphics
import CoreMedia
import AnyDisplayProtocol
import Foundation
import Network
import QuartzCore
import SystemConfiguration

public struct SessionOptions {
    /// Accept the pairing code without asking (CLI `-y`, tests).
    public var autoConfirmSAS = false
    public var clipboard = true

    public init(autoConfirmSAS: Bool = false, clipboard: Bool = true) {
        self.autoConfirmSAS = autoConfirmSAS
        self.clipboard = clipboard
    }
}

public struct StreamInfo: Sendable {
    public let deviceName: String
    public let authMode: AuthMode
    public let width: Int
    public let height: Int
    public let refreshHz: Int
    public let codec: Codec
    public let bitrateKbps: Int
    public let hardwareEncoder: Bool
    public let mirroring: Bool
    public let logicalWidth: Int
    public let logicalHeight: Int
}

public enum SessionState: Sendable {
    case connecting
    case handshaking
    /// Mode 1: both screens show this code.
    case confirmingCode(String)
    case streaming(StreamInfo)
    case closed(error: String?)
}

/// One host session with one client (PROTOCOL.md sections 4-5): handshake, stream configuration,
/// virtual display, capture, encode, flow control, input, keepalive. All state lives on `queue`;
/// callbacks are delivered on the main queue.
public final class HostSession {
    public let id = UUID()
    public let endpoint: NWEndpoint
    public var onStateChange: ((SessionState) -> Void)?
    /// Mode 1 without `autoConfirmSAS`: show the code, then call the reply with the user's answer.
    public var onConfirmCode: ((String, @escaping (Bool) -> Void) -> Void)?
    public private(set) var deviceName: String?

    private let store: PairingStore
    private let options: SessionOptions
    private let queue = DispatchQueue(label: "anydisplay.session", qos: .userInteractive)

    private enum Phase { case idle, handshake, awaitingDeviceInfo, awaitingConfigReq, streaming, closed }
    private var phase = Phase.idle
    private var connection: AnyDisplayConnection?
    private var handshake: HostHandshake?
    private var result: HandshakeResult?
    private var deviceInfo: DeviceInfo?
    private var stepTimer: DispatchWorkItem?
    private var pairingRetries = 0

    // Keepalive
    private var keepaliveTimer: DispatchSourceTimer?
    private var lastReceive = CACurrentMediaTime()
    private var pingSeq: UInt32 = 0
    public private(set) var rttMs: Double?
    public private(set) var lastStats: Stats?

    // Display / pipeline
    private var virtualDisplay: VirtualDisplay?
    private var capturer: ScreenCapturer?
    private var encoder: VideoEncoder?
    private let injector = InputInjector()
    private var cursor: CursorTracker?
    private var clipboard: ClipboardSync?
    private var streamInfo: StreamInfo?
    private var noticesSent = Set<UInt16>()

    // Flow control (PROTOCOL.md 5.2)
    private static let maxUnacked = 3
    private var streamID: UInt32 = 0
    private var nextFrameID: UInt32 = 0
    private var ackedFrames: UInt32 = 0  // highest acked frame_id + 1
    private var pendingEncodes = 0
    private var needKeyframe = true
    private var latest: CVPixelBuffer?
    private var latestPending = false
    private var lastEncodePTS = CMTime.zero
    public private(set) var framesSent: UInt64 = 0

    public init(endpoint: NWEndpoint, store: PairingStore, options: SessionOptions = SessionOptions()) {
        self.endpoint = endpoint
        self.store = store
        self.options = options
    }

    public func start() {
        queue.async { self.connect() }
    }

    /// Ends the session with `BYE`.
    public func stop(reason: ByeReason = .user) {
        queue.async { self.close(error: nil, bye: reason) }
    }

    // MARK: - Connection & handshake

    private func connect() {
        phase = .handshake
        notify(.connecting)
        let conn = AnyDisplayConnection(endpoint: endpoint, queue: queue)
        connection = conn
        handshake = HostHandshake(hostID: store.hostID, hostName: Self.hostName, hostOS: Self.hostOS,
                                  secretForDevice: { [store] in store.secret(for: $0) })
        conn.onReady = { [weak self] in
            guard let self else { return }
            Log.info("connected to \(conn.remoteDescription)")
            self.notify(.handshaking)
            self.armStepTimer()
        }
        conn.onFrame = { [weak self] frame in self?.onFrame(frame) }
        conn.onClose = { [weak self] error in self?.onConnectionClosed(error) }
        conn.start()
    }

    private func onConnectionClosed(_ error: ConnectionError?) {
        guard phase != .closed else { return }
        if pairingRetries > 0, phase == .handshake {
            retryPairing()
            return
        }
        close(error: error.map { "\($0)" } ?? "connection closed", bye: nil)
    }

    /// The client did not know our pairing (AUTH_FAIL unknown_host): forget it and reconnect for mode 1.
    private func retryPairing() {
        pairingRetries -= 1
        connection?.onClose = nil
        connection?.close()
        connection = nil
        queue.asyncAfter(deadline: .now() + 1) { [weak self] in
            guard let self, self.phase != .closed else { return }
            self.connect()
        }
    }

    private func armStepTimer() {
        stepTimer?.cancel()
        let timeout: TimeInterval
        switch phase {
        case .handshake: timeout = handshake?.stepTimeout ?? 10
        case .awaitingDeviceInfo, .awaitingConfigReq: timeout = 10
        default: return
        }
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            if self.phase == .handshake, let hs = self.handshake {
                self.apply(hs.timedOut())
            } else {
                self.close(error: "timed out waiting for DEVICE_INFO / CONFIG_REQ", bye: .error)
            }
        }
        stepTimer = work
        queue.asyncAfter(deadline: .now() + timeout, execute: work)
    }

    private func apply(_ events: [HostHandshake.Event]) {
        for event in events {
            switch event {
            case .send(let frame):
                connection?.send(frame)
            case .showSAS(let code):
                Log.info("pairing code: \(code)")
                notify(.confirmingCode(code))
                if options.autoConfirmSAS {
                    queue.async { [weak self] in self?.userConfirmed(true) }
                } else {
                    DispatchQueue.main.async {
                        guard let ask = self.onConfirmCode else {
                            self.queue.async { self.userConfirmed(false) }
                            return
                        }
                        ask(code) { ok in self.queue.async { self.userConfirmed(ok) } }
                    }
                }
            case .enableEncryption(let send, let receive):
                connection?.enableEncryption(send: send, receive: receive)
            case .completed(let r):
                handshakeCompleted(r)
            case .failed(let error):
                handshakeFailed(error)
                return
            }
        }
        if phase == .handshake { armStepTimer() }
    }

    private func userConfirmed(_ ok: Bool) {
        guard phase == .handshake, let hs = handshake else { return }
        apply(hs.userDecision(confirmed: ok))
    }

    private func handshakeFailed(_ error: HandshakeError) {
        if case .peerFailed(let reason) = error, reason == AuthFailReason.unknownHost.rawValue,
           let deviceID = handshake?.clientHello?.deviceID {
            Log.info("client does not know this host; forgetting its pairing and reconnecting to pair again")
            store.forget(deviceID: deviceID)
            pairingRetries = 5
            retryPairing()
            return
        }
        Log.error("\(error)")
        close(error: "\(error)", bye: nil, drain: true)
    }

    private func handshakeCompleted(_ r: HandshakeResult) {
        result = r
        pairingRetries = 0
        deviceName = r.deviceName
        if let secret = r.pairSecret {
            store.pair(deviceID: r.deviceID, name: r.deviceName, secret: secret)
            Log.info("paired with \(r.deviceName) (\(r.deviceID.hex))")
        }
        Log.info("handshake ok: device=\(r.deviceName) link=\(r.link) mode=\(r.mode.rawValue) encrypted=\(connection?.isEncrypted ?? false)")
        phase = .awaitingDeviceInfo
        armStepTimer()
        startKeepalive()
    }

    // MARK: - Frames

    private func onFrame(_ frame: Frame) {
        lastReceive = CACurrentMediaTime()
        if phase == .handshake {
            guard let hs = handshake else { return }
            apply(hs.receive(frame))
            return
        }
        switch Channel(rawValue: frame.channel) {
        case .control: onControl(frame)
        case .video:
            if frame.type == MessageType.frameAck, let ack = try? FrameAck(body: frame.body) { onAck(ack) }
        case .input:
            injector.handle(frame)
        default:
            break
        }
    }

    private func onControl(_ frame: Frame) {
        do {
            switch frame.type {
            case MessageType.deviceInfo:
                let info = try DeviceInfo(body: frame.body)
                deviceInfo = info
                Log.info("DEVICE_INFO \(info.screenW)x\(info.screenH) dpi=\(Int(info.xdpi))/\(Int(info.ydpi)) refresh=\(info.maxRefresh) codecs=0x\(String(info.codecs, radix: 16)) maxDecode=\(info.maxDecodeW)x\(info.maxDecodeH) touches=\(info.maxTouches) pen=0x\(String(info.penCaps, radix: 16)) audio=\(info.audio) \(info.model) \(info.osVersion)")
                if phase == .awaitingDeviceInfo {
                    phase = .awaitingConfigReq
                    armStepTimer()
                }
            case MessageType.configReq:
                let req = try ConfigReq(body: frame.body)
                Log.info("CONFIG_REQ \(req.width)x\(req.height)@\(req.refreshHz) codec=\(req.codec) kbps=\(req.bitrateKbps) q=\(req.quality) flags=0x\(String(req.flags, radix: 16))")
                stepTimer?.cancel()
                configure(req)
            case MessageType.ping:
                let ping = try Ping(body: frame.body)
                connection?.send(Pong(seq: ping.seq, echoedTimeUs: ping.senderTimeUs))
            case MessageType.pong:
                let pong = try Pong(body: frame.body)
                rttMs = Double(Self.nowUs &- pong.echoedTimeUs) / 1000
            case MessageType.stats:
                let s = try Stats(body: frame.body)
                lastStats = s
                Log.debug("STATS decoded=\(s.framesDecoded) dropped=\(s.framesDropped) decode=\(s.avgDecodeUs)us rtt=\(s.rttUs)us bytes=\(s.bytesReceived)")
            case MessageType.keyframeReq:
                requestKeyframe()
            case MessageType.clipboard:
                clipboard?.apply(try Clipboard(body: frame.body))
            case MessageType.bye:
                let reason = (try? Bye(body: frame.body))?.reason ?? 0
                Log.info("client said BYE (\(reason))")
                close(error: nil, bye: nil)
            default:
                break  // Unknown types are ignored (PROTOCOL.md section 6).
            }
        } catch {
            Log.error("bad control message 0x\(String(frame.type, radix: 16)): \(error)")
        }
    }

    // MARK: - Keepalive

    private static var nowUs: UInt64 { UInt64(Date().timeIntervalSince1970 * 1_000_000) }

    private func startKeepalive() {
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now() + 1, repeating: 1)
        t.setEventHandler { [weak self] in
            guard let self, let conn = self.connection else { return }
            if CACurrentMediaTime() - self.lastReceive > 5 {
                self.close(error: "no data from client for 5 s", bye: .error)
                return
            }
            conn.send(Ping(seq: self.pingSeq, senderTimeUs: Self.nowUs))
            self.pingSeq &+= 1
        }
        t.resume()
        keepaliveTimer = t
    }

    // MARK: - Stream configuration

    private func notice(_ code: NoticeCode, _ message: String) {
        Log.info("NOTICE \(code.rawValue): \(message)")
        connection?.send(Notice(code: code.rawValue, message: message))
    }

    private func noticeOnce(_ code: NoticeCode, _ message: String) {
        guard noticesSent.insert(code.rawValue).inserted else { return }
        notice(code, message)
    }

    private func configure(_ req: ConfigReq) {
        let info = deviceInfo ?? DeviceInfo(screenW: 1920, screenH: 1080, xdpi: 160, ydpi: 160, maxRefresh: 60, codecs: 1,
                                            maxDecodeW: 0, maxDecodeH: 0, maxTouches: 1, penCaps: 0, audio: 0,
                                            model: "", osVersion: "")
        let requested = StreamFlags(rawValue: req.flags)
        var granted = StreamFlags()

        // Size: 0,0 = screen size; fit inside the decoder limit and the encoder's 4096 edge.
        var w = Int(req.width == 0 ? info.screenW : req.width)
        var h = Int(req.height == 0 ? info.screenH : req.height)
        if w <= 0 || h <= 0 { (w, h) = (Int(info.screenW), Int(info.screenH)) }
        var limitW = 4096, limitH = 4096
        if info.maxDecodeW > 0 { limitW = min(limitW, Int(max(info.maxDecodeW, info.maxDecodeH))) }
        if info.maxDecodeH > 0 { limitH = min(limitH, Int(max(info.maxDecodeW, info.maxDecodeH))) }
        if w > limitW || h > limitH {
            let k = min(Double(limitW) / Double(w), Double(limitH) / Double(h))
            notice(.resolutionUnsupported, "\(w)x\(h) exceeds the decoder or encoder limit; streaming \(Int(Double(w) * k))x\(Int(Double(h) * k))")
            w = Int(Double(w) * k)
            h = Int(Double(h) * k)
        }
        w = max(w & ~1, 320)
        h = max(h & ~1, 240)

        var fps = Int(req.refreshHz == 0 ? 60 : req.refreshHz)
        if info.maxRefresh >= 1 { fps = min(fps, Int(info.maxRefresh.rounded())) }
        fps = min(max(fps, 1), 120)

        // Codec: honour the request when the client can decode it and we can encode it.
        let canDecode = { (c: Codec) in info.codecs & (1 << (c.rawValue - 1)) != 0 }
        var codec: Codec = req.codec == Codec.hevc.rawValue ? .hevc : .h264
        if !canDecode(codec) { codec = canDecode(.h264) ? .h264 : .hevc }

        var kbps = Int(req.bitrateKbps)
        if kbps == 0 { kbps = Self.autoBitrate(width: w, height: h, fps: fps, quality: Int(req.quality), codec: codec) }

        // Display: a virtual display unless mirroring was asked for or is the only option.
        var mirror = requested.contains(.mirror)
        var displayID = CGMainDisplayID()
        var logical = CGSize(width: w, height: h)
        if !mirror {
            if let (vd, hidpi) = setUpVirtualDisplay(info: info, width: w, height: h, fps: fps,
                                                     hidpi: requested.contains(.hidpi)) {
                displayID = vd.displayID
                logical = CGDisplayBounds(vd.displayID).size
                if hidpi { granted.insert(.hidpi) }
            } else {
                mirror = true
            }
            if mirror { notice(.virtualDisplayUnavailable, "Could not create a virtual display; mirroring the main display") }
        }
        if mirror {
            virtualDisplay = nil
            displayID = CGMainDisplayID()
            let b = CGDisplayBounds(displayID)
            logical = b.size
            // Keep the main display's aspect ratio inside the requested size.
            let k = min(Double(w) / b.width, Double(h) / b.height)
            w = max(Int(b.width * k) & ~1, 2)
            h = max(Int(b.height * k) & ~1, 2)
            granted.insert(.mirror)
        }
        let audio = requested.contains(.audio) && info.audio & 1 != 0
        if audio { granted.insert(.audio) }
        if requested.contains(.clientCursor) { granted.insert(.clientCursor) }

        teardownPipeline()
        streamID &+= 1
        nextFrameID = 0
        ackedFrames = 0
        pendingEncodes = 0
        needKeyframe = true
        latest = nil
        latestPending = false

        let sid = streamID
        let enc: VideoEncoder
        do {
            enc = try makeEncoder(codec: codec, width: w, height: h, fps: fps, kbps: kbps, streamID: sid)
        } catch {
            if codec == .hevc, canDecode(.h264),
               let fallback = try? makeEncoder(codec: .h264, width: w, height: h, fps: fps, kbps: kbps, streamID: sid) {
                codec = .h264
                enc = fallback
            } else {
                close(error: "\(error)", bye: .error)
                return
            }
        }
        encoder = enc
        if !enc.isHardware { notice(.softwareEncoder, "Hardware \(codec == .hevc ? "HEVC" : "H.264") encoder unavailable; using software") }

        var caps: HostCaps = [.nativePen]
        if options.clipboard { caps.insert(.clipboard) }
        if audio { caps.insert(.audio) }
        let config = Config(streamID: sid, width: UInt16(w), height: UInt16(h), refreshHz: UInt16(fps), codec: codec.rawValue,
                            bitrateKbps: UInt32(kbps), flags: granted.rawValue, logicalW: UInt16(logical.width),
                            logicalH: UInt16(logical.height), hostCaps: caps.rawValue)
        connection?.send(config)
        Log.info("CONFIG stream=\(sid) \(w)x\(h)@\(fps) \(codec == .hevc ? "HEVC" : "H.264") \(kbps) kbps flags=0x\(String(granted.rawValue, radix: 16)) logical=\(Int(logical.width))x\(Int(logical.height)) display=\(displayID) hw=\(enc.isHardware)")
        if audio {
            connection?.send(AudioConfig(sampleRate: UInt32(ScreenCapturer.audioSampleRate),
                                         channels: UInt8(ScreenCapturer.audioChannels), codec: 0))
        }

        if !CGPreflightScreenCaptureAccess() {
            noticeOnce(.capturePermissionMissing, "Allow Screen Recording for AnyDisplay in System Settings > Privacy & Security")
        }
        if !AXIsProcessTrusted() {
            noticeOnce(.inputPermissionMissing, "Allow Accessibility for AnyDisplay in System Settings > Privacy & Security to use touch, pen and keyboard")
        }

        let cap = ScreenCapturer(
            settings: .init(displayID: displayID, width: w, height: h, fps: fps,
                            showsCursor: !granted.contains(.clientCursor), captureAudio: audio),
            onVideo: { [weak self] pb, _ in self?.queue.async { self?.onCaptured(pb, streamID: sid) } },
            onAudio: { [weak self] sample in
                guard let pcm = AudioConverter.s16le(sample) else { return }
                let pts = UInt64(max(CMSampleBufferGetPresentationTimeStamp(sample).seconds, 0) * 1_000_000)
                self?.queue.async {
                    guard self?.streamID == sid else { return }
                    self?.connection?.send(AudioData(ptsUs: pts, samples: pcm))
                }
            },
            onStop: { [weak self] error in
                self?.queue.async {
                    guard let self, self.streamID == sid, self.phase == .streaming else { return }
                    self.close(error: "screen capture stopped: \(error.localizedDescription)", bye: .error)
                }
            })
        capturer = cap
        cap.start { [weak self] error in
            guard let error else { return }
            self?.queue.async {
                guard let self, self.streamID == sid else { return }
                Log.error("\(error)")
                self.noticeOnce(.capturePermissionMissing, "Screen capture failed: \(error)")
            }
        }

        injector.setTarget(.init(displayID: displayID, streamWidth: w, streamHeight: h))
        if granted.contains(.clientCursor) {
            let send: (Frame) -> Void = { [weak self] f in self?.queue.async { self?.connection?.send(f) } }
            let c = CursorTracker(displayID: displayID, streamWidth: w, streamHeight: h, fps: fps, send: send)
            c.start()
            cursor = c
        }
        if options.clipboard, clipboard == nil {
            let c = ClipboardSync { [weak self] f in self?.queue.async { self?.connection?.send(f) } }
            c.start()
            clipboard = c
        }

        phase = .streaming
        let si = StreamInfo(deviceName: result?.deviceName ?? "", authMode: result?.mode ?? .none, width: w, height: h,
                            refreshHz: fps, codec: codec, bitrateKbps: kbps, hardwareEncoder: enc.isHardware,
                            mirroring: mirror, logicalWidth: Int(logical.width), logicalHeight: Int(logical.height))
        streamInfo = si
        notify(.streaming(si))
    }

    /// Creates (or reuses) the virtual display at the stream size. Returns it with the HiDPI setting
    /// that macOS accepted, or nil when no virtual display could be brought up.
    private func setUpVirtualDisplay(info: DeviceInfo, width: Int, height: Int, fps: Int, hidpi: Bool) -> (VirtualDisplay, Bool)? {
        // A display identity macOS once marked inactive stays inactive; retry under another serial.
        for identity in 0..<3 {
            guard let vd = ensureVirtualDisplay(info: info, width: width, height: height, identity: identity) else { return nil }
            if vd.apply(pixelWidth: width, pixelHeight: height, hidpi: hidpi, refreshHz: Double(fps)) { return (vd, hidpi) }
            if hidpi {
                Log.info("HiDPI \(width / 2)x\(height / 2) not offered by macOS; using 1x")
                if vd.apply(pixelWidth: width, pixelHeight: height, hidpi: false, refreshHz: Double(fps)) { return (vd, false) }
            }
            virtualDisplay = nil
            guard CGDisplayIsActive(vd.displayID) == 0 else { return nil }
            Log.info("virtual display \(vd.displayID) stayed inactive; retrying with another display identity")
        }
        return nil
    }

    private func ensureVirtualDisplay(info: DeviceInfo, width: Int, height: Int, identity: Int) -> VirtualDisplay? {
        let edge = max(width, height)
        if let vd = virtualDisplay, edge <= vd.maxPixels { return vd }
        virtualDisplay = nil
        // Physical size from the client's density, in the stream's orientation.
        let xdpi = Double(info.xdpi > 1 ? info.xdpi : 160), ydpi = Double(info.ydpi > 1 ? info.ydpi : 160)
        var mmW = Double(info.screenW) / xdpi * 25.4, mmH = Double(info.screenH) / ydpi * 25.4
        if (width > height) != (mmW > mmH) { swap(&mmW, &mmH) }
        // macOS keeps a virtual display inactive above roughly 260 dpi (phone densities), and remembers
        // that for the display's identity, so the reported size never exceeds 200 dpi.
        mmW = max(mmW, Double(width) / 200 * 25.4)
        mmH = max(mmH, Double(height) / 200 * 25.4)
        // A serial derived from device_id lets macOS remember the arrangement per device.
        let deviceID = result?.deviceID ?? Data(count: 4)
        let base = deviceID.prefix(4).enumerated().reduce(UInt32(0)) { $0 | UInt32($1.element) << (8 * $1.offset) }
        let serial = base &+ UInt32(identity) &* 0x9E37_79B9
        let name = "AnyDisplay \(result?.deviceName ?? "")".trimmingCharacters(in: .whitespaces)
        let maxPixels = max(edge, Int(max(info.screenW, info.screenH)))
        Log.debug("creating virtual display \(name) serial=\(serial) mm=\(Int(mmW))x\(Int(mmH)) max=\(maxPixels)")
        let vd = VirtualDisplay(name: name, serial: serial, sizeMillimeters: CGSize(width: mmW, height: mmH), maxPixels: maxPixels)
        if vd == nil { Log.error("CGVirtualDisplay creation failed") }
        virtualDisplay = vd
        return vd
    }

    static func autoBitrate(width: Int, height: Int, fps: Int, quality: Int, codec: Codec) -> Int {
        let q = Double(quality == 0 ? 80 : min(max(quality, 1), 100)) / 100
        let bitsPerPixel = 0.03 + 0.09 * q
        var kbps = Double(width * height) * Double(min(fps, 60)) * bitsPerPixel / 1000
        if codec == .hevc { kbps *= 0.7 }
        return Int(min(max(kbps, 2000), 60000))
    }

    private func makeEncoder(codec: Codec, width: Int, height: Int, fps: Int, kbps: Int, streamID sid: UInt32) throws -> VideoEncoder {
        try VideoEncoder(codec: codec, width: width, height: height, fps: fps, bitrateKbps: kbps,
                         onOutput: { [weak self] out in self?.queue.async { self?.onEncoded(out, streamID: sid) } },
                         onDrop: { [weak self] in self?.queue.async { self?.onEncodeDropped(streamID: sid) } })
    }

    private func teardownPipeline() {
        capturer?.stop()
        capturer = nil
        encoder?.invalidate()
        encoder = nil
        cursor?.stop()
        cursor = nil
    }

    // MARK: - Video pipeline and flow control

    private var inFlight: Int { pendingEncodes + Int(nextFrameID &- ackedFrames) }

    private func onCaptured(_ pb: CVPixelBuffer, streamID sid: UInt32) {
        guard sid == streamID, phase == .streaming else { return }
        latest = pb
        latestPending = true
        submitIfPossible()
    }

    /// Encodes the newest captured frame unless 3 frames are unacknowledged; skipped captures are
    /// replaced by newer ones, so the client catches up to the current screen as soon as acks arrive.
    private func submitIfPossible() {
        guard let enc = encoder, latestPending, let pb = latest, inFlight < Self.maxUnacked else { return }
        latestPending = false
        var pts = CMTime(seconds: CACurrentMediaTime(), preferredTimescale: 1_000_000)
        if pts <= lastEncodePTS { pts = lastEncodePTS + CMTime(value: 1, timescale: 1_000_000) }
        lastEncodePTS = pts
        pendingEncodes += 1
        let key = needKeyframe
        needKeyframe = false
        enc.encode(pb, pts: pts, forceKeyframe: key)
    }

    private func onEncoded(_ out: VideoEncoder.Output, streamID sid: UInt32) {
        guard sid == streamID, let conn = connection else { return }
        pendingEncodes = max(pendingEncodes - 1, 0)
        if nextFrameID == 0, !out.keyframe {
            // The first frame of a stream must be a keyframe; drop and force one.
            needKeyframe = true
            latestPending = latest != nil
            submitIfPossible()
            return
        }
        let ptsUs = UInt64(max(out.pts.seconds, 0) * 1_000_000)
        conn.send(VideoFrame(streamID: sid, frameID: nextFrameID, ptsUs: ptsUs, keyframe: out.keyframe, accessUnit: out.accessUnit))
        if nextFrameID == 0 || out.keyframe { Log.debug("keyframe \(nextFrameID) \(out.accessUnit.count) bytes") }
        nextFrameID &+= 1
        framesSent += 1
        submitIfPossible()
    }

    private func onEncodeDropped(streamID sid: UInt32) {
        guard sid == streamID else { return }
        pendingEncodes = max(pendingEncodes - 1, 0)
        if nextFrameID == 0 { needKeyframe = true }
        submitIfPossible()
    }

    private func onAck(_ ack: FrameAck) {
        guard ack.streamID == streamID, ack.frameID < nextFrameID else { return }
        ackedFrames = max(ackedFrames, ack.frameID &+ 1)
        submitIfPossible()
    }

    private func requestKeyframe() {
        needKeyframe = true
        // A static screen produces no new captures, so re-encode the last one.
        if latest != nil { latestPending = true }
        submitIfPossible()
    }

    // MARK: - Close

    private func close(error: String?, bye: ByeReason?, drain: Bool = false) {
        guard phase != .closed else { return }
        phase = .closed
        stepTimer?.cancel()
        keepaliveTimer?.cancel()
        keepaliveTimer = nil
        teardownPipeline()
        clipboard?.stop()
        clipboard = nil
        injector.reset()
        injector.setTarget(nil)
        virtualDisplay = nil
        if let conn = connection {
            conn.onClose = nil
            if let bye, result != nil {
                conn.send(Bye(reason: bye.rawValue))
                conn.closeAfterDrain()
            } else if drain {
                conn.closeAfterDrain()
            } else {
                conn.close()
            }
        }
        connection = nil
        if let error { Log.error("session closed: \(error)") } else { Log.info("session closed") }
        Log.info("sent \(framesSent) video frames")
        notify(.closed(error: error))
    }

    private func notify(_ state: SessionState) {
        DispatchQueue.main.async { self.onStateChange?(state) }
    }

    // MARK: - Host identity

    static var hostName: String {
        (SCDynamicStoreCopyComputerName(nil, nil) as String?) ?? ProcessInfo.processInfo.hostName
    }

    static var hostOS: String {
        let v = ProcessInfo.processInfo.operatingSystemVersion
        return "macOS \(v.majorVersion).\(v.minorVersion).\(v.patchVersion)"
    }
}
