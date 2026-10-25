import Foundation

/// Persistent host identity and paired devices, stored as JSON (default
/// `~/Library/Application Support/AnyDisplay/host.json`, mode 0600 because it holds pairing secrets).
public final class PairingStore: @unchecked Sendable {
    public struct Device: Codable, Equatable, Sendable {
        public var name: String
        /// 32-byte `pair_secret`, hex.
        public var secret: String
        public var pairedAt: Date

        enum CodingKeys: String, CodingKey {
            case name, secret
            case pairedAt = "paired_at"
        }
    }

    private struct File: Codable {
        var hostID: String
        var devices: [String: Device]

        enum CodingKeys: String, CodingKey {
            case hostID = "host_id"
            case devices
        }
    }

    public static var defaultURL: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("AnyDisplay", isDirectory: true)
            .appendingPathComponent("host.json")
    }

    public let url: URL
    private let lock = NSLock()
    private var file: File

    public init(url: URL = PairingStore.defaultURL) {
        self.url = url
        if let data = try? Data(contentsOf: url),
           let decoded = try? Self.decoder.decode(File.self, from: data),
           Data(hex: decoded.hostID)?.count == 16 {
            file = decoded
        } else {
            file = File(hostID: AnyDisplayCrypto.randomBytes(16).hex, devices: [:])
            save()
        }
    }

    public var hostID: Data { lock.withLock { Data(hex: file.hostID)! } }

    public var devices: [String: Device] { lock.withLock { file.devices } }

    public func secret(for deviceID: Data) -> Data? {
        lock.withLock { file.devices[deviceID.hex].flatMap { Data(hex: $0.secret) } }
    }

    public func pair(deviceID: Data, name: String, secret: Data) {
        lock.withLock {
            file.devices[deviceID.hex] = Device(name: name, secret: secret.hex, pairedAt: Date())
        }
        save()
    }

    public func forget(deviceID: Data) {
        _ = lock.withLock { file.devices.removeValue(forKey: deviceID.hex) }
        save()
    }

    public func forgetAll() {
        lock.withLock { file.devices.removeAll() }
        save()
    }

    private static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        return d
    }()

    private func save() {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        let data = lock.withLock { try? encoder.encode(file) }
        guard let data else { return }
        let fm = FileManager.default
        let dir = url.deletingLastPathComponent()
        try? fm.createDirectory(at: dir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try? data.write(to: url, options: .atomic)
        try? fm.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }
}
