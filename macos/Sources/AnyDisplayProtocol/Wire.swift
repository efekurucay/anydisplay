import Foundation

/// Little-endian body encoder for PROTOCOL.md section 1.
public final class ByteWriter {
    public private(set) var data = Data()

    public init(capacity: Int = 64) {
        data.reserveCapacity(capacity)
    }

    @discardableResult public func u8(_ v: UInt8) -> ByteWriter { data.append(v); return self }
    @discardableResult public func u16(_ v: UInt16) -> ByteWriter { appendLE(v); return self }
    @discardableResult public func i16(_ v: Int16) -> ByteWriter { appendLE(UInt16(bitPattern: v)); return self }
    @discardableResult public func u32(_ v: UInt32) -> ByteWriter { appendLE(v); return self }
    @discardableResult public func i32(_ v: Int32) -> ByteWriter { appendLE(UInt32(bitPattern: v)); return self }
    @discardableResult public func u64(_ v: UInt64) -> ByteWriter { appendLE(v); return self }
    @discardableResult public func i64(_ v: Int64) -> ByteWriter { appendLE(UInt64(bitPattern: v)); return self }
    @discardableResult public func f32(_ v: Float) -> ByteWriter { appendLE(v.bitPattern); return self }
    @discardableResult public func raw(_ v: Data) -> ByteWriter { data.append(v); return self }
    @discardableResult public func raw(_ v: [UInt8]) -> ByteWriter { data.append(contentsOf: v); return self }

    /// `str`: u16 byte length + UTF-8. Truncates at a character boundary if longer than 65535 bytes.
    @discardableResult public func str(_ s: String) -> ByteWriter {
        var utf8 = Array(s.utf8)
        if utf8.count > Int(UInt16.max) {
            utf8 = Array(utf8.prefix(Int(UInt16.max)))
            while String(bytes: utf8, encoding: .utf8) == nil { utf8.removeLast() }
        }
        u16(UInt16(utf8.count))
        data.append(contentsOf: utf8)
        return self
    }

    /// `blob`: u32 byte length + bytes.
    @discardableResult public func blob(_ v: Data) -> ByteWriter {
        u32(UInt32(v.count))
        data.append(v)
        return self
    }

    private func appendLE<T: FixedWidthInteger>(_ v: T) {
        withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) }
    }
}

public enum WireError: Error, Equatable, Sendable {
    case shortRead
    case badUTF8
}

/// Little-endian body decoder. Trailing bytes are left unread (PROTOCOL.md section 6).
public struct ByteReader: Sendable {
    public let data: Data
    public private(set) var offset: Int

    public init(_ data: Data) {
        self.data = data
        self.offset = data.startIndex
    }

    public var remaining: Int { data.endIndex - offset }

    public mutating func u8() throws -> UInt8 { try take(1).first! }
    public mutating func u16() throws -> UInt16 { try int() }
    public mutating func i16() throws -> Int16 { Int16(bitPattern: try int()) }
    public mutating func u32() throws -> UInt32 { try int() }
    public mutating func i32() throws -> Int32 { Int32(bitPattern: try int()) }
    public mutating func u64() throws -> UInt64 { try int() }
    public mutating func i64() throws -> Int64 { Int64(bitPattern: try int()) }
    public mutating func f32() throws -> Float { Float(bitPattern: try int()) }
    public mutating func raw(_ n: Int) throws -> Data { try take(n) }

    public mutating func str() throws -> String {
        let n = Int(try u16())
        guard let s = String(data: try take(n), encoding: .utf8) else { throw WireError.badUTF8 }
        return s
    }

    public mutating func blob() throws -> Data { try take(Int(try u32())) }

    public mutating func rest() -> Data {
        let v = data[offset..<data.endIndex]
        offset = data.endIndex
        return Data(v)
    }

    private mutating func int<T: FixedWidthInteger>() throws -> T {
        let bytes = try take(MemoryLayout<T>.size)
        var v: T = 0
        for (i, b) in bytes.enumerated() { v |= T(b) << (8 * i) }
        return v
    }

    private mutating func take(_ n: Int) throws -> Data {
        guard n >= 0, remaining >= n else { throw WireError.shortRead }
        let v = data[offset..<offset + n]
        offset += n
        return Data(v)
    }
}

extension Data {
    public init?(hex: String) {
        guard hex.count % 2 == 0 else { return nil }
        var out = Data(capacity: hex.count / 2)
        var it = hex.makeIterator()
        while let a = it.next(), let b = it.next() {
            guard let byte = UInt8(String([a, b]), radix: 16) else { return nil }
            out.append(byte)
        }
        self = out
    }

    public var hex: String { map { String(format: "%02x", $0) }.joined() }
}
