import AnyDisplayProtocol
import Foundation
import Network

public enum ConnectionError: Error, CustomStringConvertible {
    case network(NWError)
    case framing(Error)
    case decryptFailed
    case closedByPeer

    public var description: String {
        switch self {
        case .network(let e): return "network: \(e)"
        case .framing(let e): return "framing: \(e)"
        case .decryptFailed: return "frame failed authentication"
        case .closedByPeer: return "connection closed by peer"
        }
    }
}

/// A framed AnyDisplay TCP connection (PROTOCOL.md sections 2-3).
///
/// Outgoing frames wait in one queue per channel; the writer drains control, input, cursor and
/// audio before video and keeps a single write in flight, so a large video frame delays small
/// messages by at most one frame write. Frames are encrypted when they are written, which keeps
/// the GCM sequence numbers in wire order. All methods must be called on `queue`.
public final class AnyDisplayConnection {
    public let queue: DispatchQueue
    public let endpoint: NWEndpoint
    private let nw: NWConnection

    private var parser = FrameParser()
    private var sendCipher: FrameCipher?
    private var receiveCipher: FrameCipher?
    private var pending: [[Frame]] = Array(repeating: [], count: Channel.allCases.count)
    private var writing = false
    private var closeWhenDrained = false
    private var finished = false
    /// Keeps the connection alive while it drains after its owner let go of it.
    private var drainRetain: AnyDisplayConnection?

    public var onReady: (() -> Void)?
    public var onFrame: ((Frame) -> Void)?
    /// Called once; nil error means a local close.
    public var onClose: ((ConnectionError?) -> Void)?

    public private(set) var bytesSent: UInt64 = 0
    public private(set) var bytesReceived: UInt64 = 0

    private static let drainOrder: [Channel] = [.control, .input, .cursor, .audio, .video]
    private static let coalesceLimit = 64 * 1024

    public init(endpoint: NWEndpoint, queue: DispatchQueue) {
        self.endpoint = endpoint
        self.queue = queue
        let tcp = NWProtocolTCP.Options()
        tcp.noDelay = true
        tcp.connectionTimeout = 10
        tcp.enableKeepalive = true
        let params = NWParameters(tls: nil, tcp: tcp)
        nw = NWConnection(to: endpoint, using: params)
    }

    public var isEncrypted: Bool { sendCipher != nil }

    public var remoteDescription: String {
        if case .hostPort(let host, let port)? = nw.currentPath?.remoteEndpoint {
            return "\(host):\(port)"
        }
        return "\(endpoint)"
    }

    public func start() {
        nw.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                self.onReady?()
                self.receive()
            case .waiting(let error):
                // A refused or unreachable TCP endpoint parks in `.waiting`; for a direct connect that is a failure.
                self.fail(.network(error))
            case .failed(let error):
                self.fail(.network(error))
            default:
                break
            }
        }
        nw.start(queue: queue)
    }

    public func send(_ frame: Frame) {
        guard !finished, !closeWhenDrained, let ch = Channel(rawValue: frame.channel) else { return }
        pending[Int(ch.rawValue)].append(frame)
        pump()
    }

    public func send<M: Message>(_ message: M) {
        send(message.frame)
    }

    /// Number of frames waiting to be written on a channel.
    public func queuedFrames(_ channel: Channel) -> Int { pending[Int(channel.rawValue)].count }

    /// Encrypt every subsequent frame (both directions). Call between frames, on `queue`.
    public func enableEncryption(send: Data, receive: Data) {
        sendCipher = FrameCipher(key: send)
        receiveCipher = FrameCipher(key: receive)
    }

    /// Close after the queued frames are written (used after BYE / AUTH_FAIL).
    public func closeAfterDrain() {
        guard !finished else { return }
        closeWhenDrained = true
        drainRetain = self
        if !writing && pending.allSatisfy(\.isEmpty) { finish(nil) }
    }

    public func close() {
        finish(nil)
    }

    // MARK: - Private

    private func receive() {
        nw.receive(minimumIncompleteLength: 1, maximumLength: 1 << 20) { [weak self] data, _, isComplete, error in
            guard let self, !self.finished else { return }
            if let data, !data.isEmpty {
                self.bytesReceived += UInt64(data.count)
                self.parser.append(data)
                do {
                    while !self.finished, var frame = try self.parser.next() {
                        if self.receiveCipher != nil {
                            do { frame = try self.receiveCipher!.open(frame) } catch {
                                self.fail(.decryptFailed)
                                return
                            }
                        }
                        self.onFrame?(frame)
                    }
                } catch {
                    self.fail(.framing(error))
                    return
                }
            }
            if let error {
                self.fail(.network(error))
            } else if isComplete {
                self.fail(.closedByPeer)
            } else if !self.finished {
                self.receive()
            }
        }
    }

    private func dequeue() -> Frame? {
        for ch in Self.drainOrder where !pending[Int(ch.rawValue)].isEmpty {
            return pending[Int(ch.rawValue)].removeFirst()
        }
        return nil
    }

    private func nextIsSmall() -> Bool {
        for ch in Self.drainOrder where !pending[Int(ch.rawValue)].isEmpty {
            return ch != .video
        }
        return false
    }

    private func encode(_ frame: Frame) -> Data {
        guard sendCipher != nil else { return frame.wireBytes }
        return sendCipher!.seal(frame).wireBytes
    }

    private func pump() {
        guard !writing, !finished else { return }
        guard let first = dequeue() else {
            if closeWhenDrained { finish(nil) }
            return
        }
        var chunk = encode(first)
        // Coalesce small non-video frames into one write.
        if first.channel != Channel.video.rawValue {
            while chunk.count < Self.coalesceLimit, nextIsSmall(), let next = dequeue() {
                chunk.append(encode(next))
            }
        }
        writing = true
        bytesSent += UInt64(chunk.count)
        nw.send(content: chunk, completion: .contentProcessed { [weak self] error in
            guard let self else { return }
            self.writing = false
            if let error {
                self.fail(.network(error))
                return
            }
            self.pump()
        })
    }

    private func fail(_ error: ConnectionError) {
        finish(error)
    }

    private func finish(_ error: ConnectionError?) {
        guard !finished else { return }
        finished = true
        pending = Array(repeating: [], count: Channel.allCases.count)
        nw.stateUpdateHandler = nil
        nw.cancel()
        let cb = onClose
        onClose = nil
        onFrame = nil
        onReady = nil
        cb?(error)
        drainRetain = nil
    }
}
