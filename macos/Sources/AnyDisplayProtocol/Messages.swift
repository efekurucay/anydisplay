import Foundation

public let anyDisplayPort: UInt16 = 47820
public let anyDisplayMagic = Data("ANYD".utf8)
public let anyDisplayMajor: UInt16 = 1
public let anyDisplayMinor: UInt16 = 0
public let anyDisplayBonjourType = "_anydisplay._tcp"

public enum Channel: UInt8, Sendable, CaseIterable {
    case control = 0, video = 1, audio = 2, cursor = 3, input = 4
}

public enum MessageType {
    // Handshake (channel 0, never encrypted)
    public static let helloC: UInt8 = 0x01
    public static let helloH: UInt8 = 0x02
    public static let authH: UInt8 = 0x03
    public static let authC: UInt8 = 0x04
    public static let authFail: UInt8 = 0x05
    public static let sasOK: UInt8 = 0x06
    public static let pair: UInt8 = 0x07
    public static let reveal: UInt8 = 0x08
    // Control
    public static let deviceInfo: UInt8 = 0x10
    public static let configReq: UInt8 = 0x11
    public static let config: UInt8 = 0x12
    public static let ping: UInt8 = 0x13
    public static let pong: UInt8 = 0x14
    public static let stats: UInt8 = 0x15
    public static let keyframeReq: UInt8 = 0x16
    public static let notice: UInt8 = 0x17
    public static let clipboard: UInt8 = 0x18
    public static let bye: UInt8 = 0x19
    // Video
    public static let videoFrame: UInt8 = 0x20
    public static let frameAck: UInt8 = 0x21
    // Cursor
    public static let cursorPos: UInt8 = 0x30
    public static let cursorShape: UInt8 = 0x31
    // Audio
    public static let audioConfig: UInt8 = 0x40
    public static let audioData: UInt8 = 0x41
    // Input
    public static let touch: UInt8 = 0x50
    public static let pen: UInt8 = 0x51
    public static let key: UInt8 = 0x52
    public static let text: UInt8 = 0x53
    public static let scroll: UInt8 = 0x54
    public static let mouse: UInt8 = 0x55
}

public enum AuthFailReason: UInt8, Sendable, Error {
    case badVersion = 1, authRequired = 2, unknownHost = 3, badMAC = 4, userRejected = 5, busy = 6, badCommit = 7
}

public enum AuthMode: UInt8, Sendable {
    case none = 0, sas = 1, paired = 2
}

public enum Link: UInt8, Sendable {
    case loopback = 0, network = 1, usbAccessory = 2
}

public enum Codec: UInt8, Sendable {
    case h264 = 1, hevc = 2, av1 = 3
}

public enum NoticeCode: UInt16, Sendable {
    case softwareEncoder = 1
    case capturePermissionMissing = 2
    case inputPermissionMissing = 3
    case resolutionUnsupported = 4
    case virtualDisplayUnavailable = 5
}

public enum ByeReason: UInt8, Sendable {
    case user = 0, shutdown = 1, error = 2
}

/// A message body type with a fixed channel and type byte.
public protocol Message: Sendable {
    static var channel: Channel { get }
    static var type: UInt8 { get }
    func encode() -> Data
    init(body: Data) throws
}

extension Message {
    public var frame: Frame { Frame(channel: Self.channel.rawValue, type: Self.type, body: encode()) }
}

// MARK: - Handshake

public struct HelloC: Message {
    public static let channel = Channel.control
    public static let type = MessageType.helloC
    public var magic: Data
    public var major: UInt16
    public var minor: UInt16
    public var deviceID: Data
    public var deviceName: String
    public var link: UInt8
    public var authRequired: UInt8
    public var clientPub: Data
    public var clientCommit: Data

    public init(magic: Data = anyDisplayMagic, major: UInt16 = anyDisplayMajor, minor: UInt16 = anyDisplayMinor, deviceID: Data,
                deviceName: String, link: UInt8, authRequired: UInt8, clientPub: Data, clientCommit: Data) {
        self.magic = magic; self.major = major; self.minor = minor; self.deviceID = deviceID
        self.deviceName = deviceName; self.link = link; self.authRequired = authRequired
        self.clientPub = clientPub; self.clientCommit = clientCommit
    }

    public init(body: Data) throws {
        var r = ByteReader(body)
        magic = try r.raw(4)
        major = try r.u16()
        minor = try r.u16()
        deviceID = try r.raw(16)
        deviceName = try r.str()
        link = try r.u8()
        authRequired = try r.u8()
        clientPub = try r.raw(65)
        clientCommit = try r.raw(32)
    }

    public func encode() -> Data {
        let w = ByteWriter()
        w.raw(magic).u16(major).u16(minor).raw(deviceID).str(deviceName).u8(link).u8(authRequired)
            .raw(clientPub).raw(clientCommit)
        return w.data
    }
}

public struct HelloH: Message {
    public static let channel = Channel.control
    public static let type = MessageType.helloH
    public var magic: Data
    public var major: UInt16
    public var minor: UInt16
    public var hostID: Data
    public var hostName: String
    public var hostOS: String
    public var authMode: UInt8
    public var hostPub: Data
    public var hostNonce: Data

    public init(magic: Data = anyDisplayMagic, major: UInt16 = anyDisplayMajor, minor: UInt16 = anyDisplayMinor, hostID: Data,
                hostName: String, hostOS: String, authMode: UInt8, hostPub: Data, hostNonce: Data) {
        self.magic = magic; self.major = major; self.minor = minor; self.hostID = hostID
        self.hostName = hostName; self.hostOS = hostOS; self.authMode = authMode
        self.hostPub = hostPub; self.hostNonce = hostNonce
    }

    public init(body: Data) throws {
        var r = ByteReader(body)
        magic = try r.raw(4)
        major = try r.u16()
        minor = try r.u16()
        hostID = try r.raw(16)
        hostName = try r.str()
        hostOS = try r.str()
        authMode = try r.u8()
        hostPub = try r.raw(65)
        hostNonce = try r.raw(32)
    }

    public func encode() -> Data {
        let w = ByteWriter()
        w.raw(magic).u16(major).u16(minor).raw(hostID).str(hostName).str(hostOS).u8(authMode)
            .raw(hostPub).raw(hostNonce)
        return w.data
    }
}

// MARK: - Control

public struct DeviceInfo: Message, Equatable {
    public static let channel = Channel.control
    public static let type = MessageType.deviceInfo
    public var screenW: UInt16
    public var screenH: UInt16
    public var xdpi: Float
    public var ydpi: Float
    public var maxRefresh: Float
    public var codecs: UInt32
    public var maxDecodeW: UInt16
    public var maxDecodeH: UInt16
    public var maxTouches: UInt8
    public var penCaps: UInt32
    public var audio: UInt8
    public var model: String
    public var osVersion: String

    public init(screenW: UInt16, screenH: UInt16, xdpi: Float, ydpi: Float, maxRefresh: Float, codecs: UInt32,
                maxDecodeW: UInt16, maxDecodeH: UInt16, maxTouches: UInt8, penCaps: UInt32, audio: UInt8,
                model: String, osVersion: String) {
        self.screenW = screenW; self.screenH = screenH; self.xdpi = xdpi; self.ydpi = ydpi
        self.maxRefresh = maxRefresh; self.codecs = codecs; self.maxDecodeW = maxDecodeW
        self.maxDecodeH = maxDecodeH; self.maxTouches = maxTouches; self.penCaps = penCaps
        self.audio = audio; self.model = model; self.osVersion = osVersion
    }

    public init(body: Data) throws {
        var r = ByteReader(body)
        screenW = try r.u16(); screenH = try r.u16()
        xdpi = try r.f32(); ydpi = try r.f32(); maxRefresh = try r.f32()
        codecs = try r.u32()
        maxDecodeW = try r.u16(); maxDecodeH = try r.u16()
        maxTouches = try r.u8()
        penCaps = try r.u32()
        audio = try r.u8()
        model = try r.str(); osVersion = try r.str()
    }

    public func encode() -> Data {
        let w = ByteWriter()
        w.u16(screenW).u16(screenH).f32(xdpi).f32(ydpi).f32(maxRefresh).u32(codecs).u16(maxDecodeW)
            .u16(maxDecodeH).u8(maxTouches).u32(penCaps).u8(audio).str(model).str(osVersion)
        return w.data
    }
}

public struct StreamFlags: OptionSet, Sendable {
    public let rawValue: UInt8
    public init(rawValue: UInt8) { self.rawValue = rawValue }
    public static let hidpi = StreamFlags(rawValue: 1 << 0)
    public static let audio = StreamFlags(rawValue: 1 << 1)
    public static let clientCursor = StreamFlags(rawValue: 1 << 2)
    public static let mirror = StreamFlags(rawValue: 1 << 3)
}

public struct ConfigReq: Message, Equatable {
    public static let channel = Channel.control
    public static let type = MessageType.configReq
    public var width: UInt16
    public var height: UInt16
    public var refreshHz: UInt16
    public var codec: UInt8
    public var bitrateKbps: UInt32
    public var quality: UInt8
    public var flags: UInt8

    public init(width: UInt16, height: UInt16, refreshHz: UInt16, codec: UInt8, bitrateKbps: UInt32, quality: UInt8, flags: UInt8) {
        self.width = width; self.height = height; self.refreshHz = refreshHz; self.codec = codec
        self.bitrateKbps = bitrateKbps; self.quality = quality; self.flags = flags
    }

    public init(body: Data) throws {
        var r = ByteReader(body)
        width = try r.u16(); height = try r.u16(); refreshHz = try r.u16()
        codec = try r.u8(); bitrateKbps = try r.u32(); quality = try r.u8(); flags = try r.u8()
    }

    public func encode() -> Data {
        let w = ByteWriter()
        w.u16(width).u16(height).u16(refreshHz).u8(codec).u32(bitrateKbps).u8(quality).u8(flags)
        return w.data
    }
}

public struct HostCaps: OptionSet, Sendable {
    public let rawValue: UInt32
    public init(rawValue: UInt32) { self.rawValue = rawValue }
    public static let nativeTouch = HostCaps(rawValue: 1 << 0)
    public static let nativePen = HostCaps(rawValue: 1 << 1)
    public static let clipboard = HostCaps(rawValue: 1 << 2)
    public static let audio = HostCaps(rawValue: 1 << 3)
}

public struct Config: Message, Equatable {
    public static let channel = Channel.control
    public static let type = MessageType.config
    public var streamID: UInt32
    public var width: UInt16
    public var height: UInt16
    public var refreshHz: UInt16
    public var codec: UInt8
    public var bitrateKbps: UInt32
    public var flags: UInt8
    public var logicalW: UInt16
    public var logicalH: UInt16
    public var hostCaps: UInt32

    public init(streamID: UInt32, width: UInt16, height: UInt16, refreshHz: UInt16, codec: UInt8, bitrateKbps: UInt32,
                flags: UInt8, logicalW: UInt16, logicalH: UInt16, hostCaps: UInt32) {
        self.streamID = streamID; self.width = width; self.height = height; self.refreshHz = refreshHz
        self.codec = codec; self.bitrateKbps = bitrateKbps; self.flags = flags
        self.logicalW = logicalW; self.logicalH = logicalH; self.hostCaps = hostCaps
    }

    public init(body: Data) throws {
        var r = ByteReader(body)
        streamID = try r.u32(); width = try r.u16(); height = try r.u16(); refreshHz = try r.u16()
        codec = try r.u8(); bitrateKbps = try r.u32(); flags = try r.u8()
        logicalW = try r.u16(); logicalH = try r.u16(); hostCaps = try r.u32()
    }

    public func encode() -> Data {
        let w = ByteWriter()
        w.u32(streamID).u16(width).u16(height).u16(refreshHz).u8(codec).u32(bitrateKbps).u8(flags)
            .u16(logicalW).u16(logicalH).u32(hostCaps)
        return w.data
    }
}

public struct Ping: Message, Equatable {
    public static let channel = Channel.control
    public static let type = MessageType.ping
    public var seq: UInt32
    public var senderTimeUs: UInt64
    public init(seq: UInt32, senderTimeUs: UInt64) { self.seq = seq; self.senderTimeUs = senderTimeUs }
    public init(body: Data) throws { var r = ByteReader(body); seq = try r.u32(); senderTimeUs = try r.u64() }
    public func encode() -> Data { let w = ByteWriter(); w.u32(seq).u64(senderTimeUs); return w.data }
}

public struct Pong: Message, Equatable {
    public static let channel = Channel.control
    public static let type = MessageType.pong
    public var seq: UInt32
    public var echoedTimeUs: UInt64
    public init(seq: UInt32, echoedTimeUs: UInt64) { self.seq = seq; self.echoedTimeUs = echoedTimeUs }
    public init(body: Data) throws { var r = ByteReader(body); seq = try r.u32(); echoedTimeUs = try r.u64() }
    public func encode() -> Data { let w = ByteWriter(); w.u32(seq).u64(echoedTimeUs); return w.data }
}

public struct Stats: Message, Equatable {
    public static let channel = Channel.control
    public static let type = MessageType.stats
    public var framesDecoded: UInt32
    public var framesDropped: UInt32
    public var avgDecodeUs: UInt32
    public var rttUs: UInt32
    public var bytesReceived: UInt32
    public init(framesDecoded: UInt32, framesDropped: UInt32, avgDecodeUs: UInt32, rttUs: UInt32, bytesReceived: UInt32) {
        self.framesDecoded = framesDecoded; self.framesDropped = framesDropped; self.avgDecodeUs = avgDecodeUs
        self.rttUs = rttUs; self.bytesReceived = bytesReceived
    }
    public init(body: Data) throws {
        var r = ByteReader(body)
        framesDecoded = try r.u32(); framesDropped = try r.u32(); avgDecodeUs = try r.u32()
        rttUs = try r.u32(); bytesReceived = try r.u32()
    }
    public func encode() -> Data {
        let w = ByteWriter(); w.u32(framesDecoded).u32(framesDropped).u32(avgDecodeUs).u32(rttUs).u32(bytesReceived)
        return w.data
    }
}

public struct Notice: Message, Equatable {
    public static let channel = Channel.control
    public static let type = MessageType.notice
    public var code: UInt16
    public var message: String
    public init(code: UInt16, message: String) { self.code = code; self.message = message }
    public init(body: Data) throws { var r = ByteReader(body); code = try r.u16(); message = try r.str() }
    public func encode() -> Data { let w = ByteWriter(); w.u16(code).str(message); return w.data }
}

public struct Clipboard: Message, Equatable {
    public static let channel = Channel.control
    public static let type = MessageType.clipboard
    public var mime: String
    public var data: Data
    public init(mime: String, data: Data) { self.mime = mime; self.data = data }
    public init(body: Data) throws { var r = ByteReader(body); mime = try r.str(); data = try r.blob() }
    public func encode() -> Data { let w = ByteWriter(); w.str(mime).blob(data); return w.data }
}

public struct Bye: Message, Equatable {
    public static let channel = Channel.control
    public static let type = MessageType.bye
    public var reason: UInt8
    public init(reason: UInt8) { self.reason = reason }
    public init(body: Data) throws { var r = ByteReader(body); reason = try r.u8() }
    public func encode() -> Data { Data([reason]) }
}

// MARK: - Video

public struct VideoFrame: Message {
    public static let channel = Channel.video
    public static let type = MessageType.videoFrame
    public var streamID: UInt32
    public var frameID: UInt32
    public var ptsUs: UInt64
    public var flags: UInt8
    public var accessUnit: Data
    public var isKeyframe: Bool { flags & 1 != 0 }

    public init(streamID: UInt32, frameID: UInt32, ptsUs: UInt64, keyframe: Bool, accessUnit: Data) {
        self.streamID = streamID; self.frameID = frameID; self.ptsUs = ptsUs
        self.flags = keyframe ? 1 : 0; self.accessUnit = accessUnit
    }

    public init(body: Data) throws {
        var r = ByteReader(body)
        streamID = try r.u32(); frameID = try r.u32(); ptsUs = try r.u64(); flags = try r.u8()
        accessUnit = r.rest()
    }

    public func encode() -> Data {
        let w = ByteWriter(capacity: 17 + accessUnit.count)
        w.u32(streamID).u32(frameID).u64(ptsUs).u8(flags).raw(accessUnit)
        return w.data
    }
}

public struct FrameAck: Message, Equatable {
    public static let channel = Channel.video
    public static let type = MessageType.frameAck
    public var streamID: UInt32
    public var frameID: UInt32
    public var decodeUs: UInt32
    public init(streamID: UInt32, frameID: UInt32, decodeUs: UInt32) {
        self.streamID = streamID; self.frameID = frameID; self.decodeUs = decodeUs
    }
    public init(body: Data) throws {
        var r = ByteReader(body); streamID = try r.u32(); frameID = try r.u32(); decodeUs = try r.u32()
    }
    public func encode() -> Data { let w = ByteWriter(); w.u32(streamID).u32(frameID).u32(decodeUs); return w.data }
}

// MARK: - Audio

public struct AudioConfig: Message, Equatable {
    public static let channel = Channel.audio
    public static let type = MessageType.audioConfig
    public var sampleRate: UInt32
    public var channels: UInt8
    public var codec: UInt8
    public init(sampleRate: UInt32, channels: UInt8, codec: UInt8) {
        self.sampleRate = sampleRate; self.channels = channels; self.codec = codec
    }
    public init(body: Data) throws {
        var r = ByteReader(body); sampleRate = try r.u32(); channels = try r.u8(); codec = try r.u8()
    }
    public func encode() -> Data { let w = ByteWriter(); w.u32(sampleRate).u8(channels).u8(codec); return w.data }
}

public struct AudioData: Message, Equatable {
    public static let channel = Channel.audio
    public static let type = MessageType.audioData
    public var ptsUs: UInt64
    public var samples: Data
    public init(ptsUs: UInt64, samples: Data) { self.ptsUs = ptsUs; self.samples = samples }
    public init(body: Data) throws { var r = ByteReader(body); ptsUs = try r.u64(); samples = r.rest() }
    public func encode() -> Data { let w = ByteWriter(capacity: 8 + samples.count); w.u64(ptsUs).raw(samples); return w.data }
}

// MARK: - Cursor

public struct CursorPos: Message, Equatable {
    public static let channel = Channel.cursor
    public static let type = MessageType.cursorPos
    public var x: Int16
    public var y: Int16
    public var visible: UInt8
    public init(x: Int16, y: Int16, visible: Bool) { self.x = x; self.y = y; self.visible = visible ? 1 : 0 }
    public init(body: Data) throws { var r = ByteReader(body); x = try r.i16(); y = try r.i16(); visible = try r.u8() }
    public func encode() -> Data { let w = ByteWriter(); w.i16(x).i16(y).u8(visible); return w.data }
}

public struct CursorShape: Message, Equatable {
    public static let channel = Channel.cursor
    public static let type = MessageType.cursorShape
    public var w: UInt16
    public var h: UInt16
    public var hotX: UInt16
    public var hotY: UInt16
    public var rgba: Data
    public init(w: UInt16, h: UInt16, hotX: UInt16, hotY: UInt16, rgba: Data) {
        self.w = w; self.h = h; self.hotX = hotX; self.hotY = hotY; self.rgba = rgba
    }
    public init(body: Data) throws {
        var r = ByteReader(body)
        w = try r.u16(); h = try r.u16(); hotX = try r.u16(); hotY = try r.u16(); rgba = r.rest()
    }
    public func encode() -> Data {
        let b = ByteWriter(capacity: 8 + rgba.count); b.u16(w).u16(h).u16(hotX).u16(hotY).raw(rgba); return b.data
    }
}

// MARK: - Input

public struct TouchContact: Sendable, Equatable {
    public var id: UInt8
    public var action: UInt8  // 0 down, 1 move, 2 up, 3 cancel
    public var x: UInt16
    public var y: UInt16
    public var pressure: UInt16
    public var size: UInt16
    public init(id: UInt8, action: UInt8, x: UInt16, y: UInt16, pressure: UInt16, size: UInt16) {
        self.id = id; self.action = action; self.x = x; self.y = y; self.pressure = pressure; self.size = size
    }
}

public struct Touch: Message, Equatable {
    public static let channel = Channel.input
    public static let type = MessageType.touch
    public var contacts: [TouchContact]
    public init(contacts: [TouchContact]) { self.contacts = contacts }
    public init(body: Data) throws {
        var r = ByteReader(body)
        let n = Int(try r.u8())
        contacts = try (0..<n).map { _ in
            TouchContact(id: try r.u8(), action: try r.u8(), x: try r.u16(), y: try r.u16(),
                         pressure: try r.u16(), size: try r.u16())
        }
    }
    public func encode() -> Data {
        let w = ByteWriter()
        w.u8(UInt8(contacts.count))
        for c in contacts { w.u8(c.id).u8(c.action).u16(c.x).u16(c.y).u16(c.pressure).u16(c.size) }
        return w.data
    }
}

public struct PenSample: Sendable, Equatable {
    public var action: UInt8  // 0 down, 1 move, 2 up, 3 hover_enter, 4 hover_move, 5 hover_exit, 6 cancel
    public var tool: UInt8    // 0 pen, 1 eraser
    public var buttons: UInt8 // bit0 barrel, bit1 secondary
    public var x: UInt16
    public var y: UInt16
    public var pressure: UInt16
    public var tiltX: Int16
    public var tiltY: Int16
    public var twist: UInt16
    public var timeMs: UInt32
    public init(action: UInt8, tool: UInt8, buttons: UInt8, x: UInt16, y: UInt16, pressure: UInt16,
                tiltX: Int16, tiltY: Int16, twist: UInt16, timeMs: UInt32) {
        self.action = action; self.tool = tool; self.buttons = buttons; self.x = x; self.y = y
        self.pressure = pressure; self.tiltX = tiltX; self.tiltY = tiltY; self.twist = twist; self.timeMs = timeMs
    }
}

public struct Pen: Message, Equatable {
    public static let channel = Channel.input
    public static let type = MessageType.pen
    public var samples: [PenSample]
    public init(samples: [PenSample]) { self.samples = samples }
    public init(body: Data) throws {
        var r = ByteReader(body)
        let n = Int(try r.u8())
        samples = try (0..<n).map { _ in
            PenSample(action: try r.u8(), tool: try r.u8(), buttons: try r.u8(), x: try r.u16(), y: try r.u16(),
                      pressure: try r.u16(), tiltX: try r.i16(), tiltY: try r.i16(), twist: try r.u16(),
                      timeMs: try r.u32())
        }
    }
    public func encode() -> Data {
        let w = ByteWriter()
        w.u8(UInt8(samples.count))
        for s in samples {
            w.u8(s.action).u8(s.tool).u8(s.buttons).u16(s.x).u16(s.y).u16(s.pressure).i16(s.tiltX).i16(s.tiltY)
                .u16(s.twist).u32(s.timeMs)
        }
        return w.data
    }
}

public struct Key: Message, Equatable {
    public static let channel = Channel.input
    public static let type = MessageType.key
    public var action: UInt8  // 0 up, 1 down
    public var hidUsage: UInt16
    public var modifiers: UInt16  // bit0 ctrl, bit1 shift, bit2 alt, bit3 meta
    public init(action: UInt8, hidUsage: UInt16, modifiers: UInt16) {
        self.action = action; self.hidUsage = hidUsage; self.modifiers = modifiers
    }
    public init(body: Data) throws {
        var r = ByteReader(body); action = try r.u8(); hidUsage = try r.u16(); modifiers = try r.u16()
    }
    public func encode() -> Data { let w = ByteWriter(); w.u8(action).u16(hidUsage).u16(modifiers); return w.data }
}

public struct Text: Message, Equatable {
    public static let channel = Channel.input
    public static let type = MessageType.text
    public var text: String
    public init(text: String) { self.text = text }
    public init(body: Data) throws { var r = ByteReader(body); text = try r.str() }
    public func encode() -> Data { let w = ByteWriter(); w.str(text); return w.data }
}

public struct Scroll: Message, Equatable {
    public static let channel = Channel.input
    public static let type = MessageType.scroll
    public var x: UInt16
    public var y: UInt16
    public var dx: Int16
    public var dy: Int16
    public var phase: UInt8  // 0 begin, 1 change, 2 end, 3 momentum
    public init(x: UInt16, y: UInt16, dx: Int16, dy: Int16, phase: UInt8) {
        self.x = x; self.y = y; self.dx = dx; self.dy = dy; self.phase = phase
    }
    public init(body: Data) throws {
        var r = ByteReader(body); x = try r.u16(); y = try r.u16(); dx = try r.i16(); dy = try r.i16(); phase = try r.u8()
    }
    public func encode() -> Data { let w = ByteWriter(); w.u16(x).u16(y).i16(dx).i16(dy).u8(phase); return w.data }
}

public struct Mouse: Message, Equatable {
    public static let channel = Channel.input
    public static let type = MessageType.mouse
    public var action: UInt8  // 0 move, 1 down, 2 up
    public var button: UInt8  // 0 left, 1 right, 2 middle
    public var x: UInt16
    public var y: UInt16
    public init(action: UInt8, button: UInt8, x: UInt16, y: UInt16) {
        self.action = action; self.button = button; self.x = x; self.y = y
    }
    public init(body: Data) throws {
        var r = ByteReader(body); action = try r.u8(); button = try r.u8(); x = try r.u16(); y = try r.u16()
    }
    public func encode() -> Data { let w = ByteWriter(); w.u8(action).u8(button).u16(x).u16(y); return w.data }
}
