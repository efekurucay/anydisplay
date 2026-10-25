import Foundation

public let anyDisplayMaxFrameLength = 32 * 1024 * 1024

/// One wire frame: `u32 length | u8 channel | u8 type | body` (PROTOCOL.md section 3).
public struct Frame: Sendable, Equatable {
    public var channel: UInt8
    public var type: UInt8
    public var body: Data

    public init(channel: UInt8, type: UInt8, body: Data = Data()) {
        self.channel = channel
        self.type = type
        self.body = body
    }

    public init(_ channel: Channel, _ type: UInt8, _ body: Data = Data()) {
        self.init(channel: channel.rawValue, type: type, body: body)
    }

    /// The bytes on the wire for this frame as-is (the body must already be encrypted if the session is).
    public var wireBytes: Data {
        var out = Data(capacity: 6 + body.count)
        let length = UInt32(2 + body.count)
        withUnsafeBytes(of: length.littleEndian) { out.append(contentsOf: $0) }
        out.append(channel)
        out.append(type)
        out.append(body)
        return out
    }

    public var aad: Data { Data([channel, type]) }
}

public enum FramingError: Error, Equatable, Sendable {
    case badLength(UInt32)
}

/// Incremental parser: feed TCP bytes, pull complete frames.
public struct FrameParser: Sendable {
    private var buffer = Data()

    public init() {}

    public mutating func append(_ bytes: Data) {
        buffer.append(bytes)
    }

    /// Returns the next complete frame, or nil if more bytes are needed.
    public mutating func next() throws -> Frame? {
        guard buffer.count >= 6 else { return nil }
        let s = buffer.startIndex
        let length = UInt32(buffer[s]) | UInt32(buffer[s + 1]) << 8 | UInt32(buffer[s + 2]) << 16 | UInt32(buffer[s + 3]) << 24
        guard length >= 2, length <= UInt32(anyDisplayMaxFrameLength) else { throw FramingError.badLength(length) }
        let total = 4 + Int(length)
        guard buffer.count >= total else { return nil }
        let frame = Frame(channel: buffer[s + 4], type: buffer[s + 5], body: Data(buffer[(s + 6)..<(s + total)]))
        buffer = buffer.count == total ? Data() : Data(buffer[(s + total)...])
        return frame
    }
}
