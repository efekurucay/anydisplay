import AnyDisplayProtocol
import Foundation
import Network

/// A client found on the local network via `_anydisplay._tcp`.
public struct NetworkDevice: Hashable, Sendable {
    public let name: String
    /// TXT `id`: the client's 32-hex-char device_id.
    public let deviceID: String?
    public let endpoint: NWEndpoint
}

/// Browses `_anydisplay._tcp` with NWBrowser. Callbacks arrive on the main queue.
public final class BonjourBrowser {
    private var browser: NWBrowser?
    public var onChange: (([NetworkDevice]) -> Void)?
    public private(set) var devices: [NetworkDevice] = []

    public init() {}

    public func start() {
        let params = NWParameters()
        params.includePeerToPeer = false
        let b = NWBrowser(for: .bonjourWithTXTRecord(type: anyDisplayBonjourType, domain: nil), using: params)
        b.stateUpdateHandler = { state in
            if case .failed(let error) = state { Log.error("bonjour browser failed: \(error)") }
            if case .waiting(let error) = state { Log.error("bonjour browser waiting: \(error)") }
        }
        b.browseResultsChangedHandler = { [weak self] results, _ in
            guard let self else { return }
            self.devices = results.compactMap { result in
                guard case .service(let name, _, _, _) = result.endpoint else { return nil }
                var id: String?
                if case .bonjour(let txt) = result.metadata { id = txt["id"] }
                return NetworkDevice(name: name, deviceID: id, endpoint: result.endpoint)
            }.sorted { $0.name < $1.name }
            self.onChange?(self.devices)
        }
        b.start(queue: .main)
        browser = b
    }

    public func stop() {
        browser?.cancel()
        browser = nil
    }
}

/// An Android device attached over USB with debugging enabled.
public struct UsbDevice: Hashable, Sendable {
    public let serial: String
    public let model: String?
}

/// Talks to `adb`: lists attached devices and sets up `adb forward tcp:<local> tcp:47820`.
public final class Adb {
    public let path: String

    /// `ANYDISPLAY_ADB` overrides the search; then PATH, the Android SDK default, and Homebrew.
    public static func locate() -> String? {
        let fm = FileManager.default
        if let env = ProcessInfo.processInfo.environment["ANYDISPLAY_ADB"], fm.isExecutableFile(atPath: env) { return env }
        let pathDirs = (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":").map(String.init)
        let candidates = pathDirs.map { $0 + "/adb" } + [
            NSHomeDirectory() + "/Library/Android/sdk/platform-tools/adb",
            "/opt/homebrew/bin/adb",
            "/usr/local/bin/adb",
        ]
        return candidates.first { fm.isExecutableFile(atPath: $0) }
    }

    public init?(path: String? = Adb.locate()) {
        guard let path else { return nil }
        self.path = path
    }

    public func devices() -> [UsbDevice] {
        guard let out = run(["devices", "-l"]) else { return [] }
        return Self.parseDevices(out)
    }

    /// Parses `adb devices -l`, keeping only devices in the `device` state (authorized and online).
    public static func parseDevices(_ output: String) -> [UsbDevice] {
        output.split(separator: "\n").compactMap { line in
            let parts = line.split(whereSeparator: { $0 == " " || $0 == "\t" }).map(String.init)
            guard parts.count >= 2, parts[1] == "device" else { return nil }
            let model = parts.dropFirst(2).first { $0.hasPrefix("model:") }.map { String($0.dropFirst(6)) }
            return UsbDevice(serial: parts[0], model: model)
        }
    }

    /// Forwards a free local port to the device's 47820 and returns it.
    public func forward(serial: String, remotePort: UInt16 = anyDisplayPort) -> UInt16? {
        guard let out = run(["-s", serial, "forward", "tcp:0", "tcp:\(remotePort)"]) else { return nil }
        return UInt16(out.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    public func removeForward(serial: String, localPort: UInt16) {
        _ = run(["-s", serial, "forward", "--remove", "tcp:\(localPort)"])
    }

    private func run(_ args: [String]) -> String? {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: path)
        p.arguments = args
        let out = Pipe()
        p.standardOutput = out
        p.standardError = Pipe()
        do { try p.run() } catch {
            Log.error("adb \(args.joined(separator: " ")): \(error)")
            return nil
        }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        guard p.terminationStatus == 0 else {
            Log.debug("adb \(args.joined(separator: " ")) exited \(p.terminationStatus)")
            return nil
        }
        return String(decoding: data, as: UTF8.self)
    }
}

/// Polls `adb devices` and reports changes on the main queue.
public final class UsbMonitor {
    private let adb: Adb
    private let queue = DispatchQueue(label: "anydisplay.adb")
    private var timer: DispatchSourceTimer?
    private var last: [UsbDevice] = []
    public var onChange: (([UsbDevice]) -> Void)?

    public init(adb: Adb) {
        self.adb = adb
    }

    public func start(interval: TimeInterval = 2) {
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now(), repeating: interval)
        t.setEventHandler { [weak self] in
            guard let self else { return }
            let now = self.adb.devices()
            guard now != self.last else { return }
            self.last = now
            DispatchQueue.main.async { self.onChange?(now) }
        }
        t.resume()
        timer = t
    }

    public func stop() {
        timer?.cancel()
        timer = nil
    }
}
