import AppKit
import AnyDisplayProtocol
import Foundation

/// Two-way `text/plain` clipboard (PROTOCOL.md 5.1 CLIPBOARD). Only changes made during the
/// session are sent; what was on the clipboard before connecting stays local.
final class ClipboardSync {
    static let mime = "text/plain"
    private let send: (Frame) -> Void
    private var timer: Timer?
    private var lastChangeCount = 0

    init(send: @escaping (Frame) -> Void) {
        self.send = send
    }

    func start() {
        DispatchQueue.main.async {
            self.lastChangeCount = NSPasteboard.general.changeCount
            self.timer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in self?.poll() }
        }
    }

    func stop() {
        DispatchQueue.main.async {
            self.timer?.invalidate()
            self.timer = nil
        }
    }

    func apply(_ clip: Clipboard) {
        guard clip.mime == Self.mime, let text = String(data: clip.data, encoding: .utf8) else { return }
        DispatchQueue.main.async {
            let pb = NSPasteboard.general
            pb.clearContents()
            pb.setString(text, forType: .string)
            self.lastChangeCount = pb.changeCount
        }
    }

    private func poll() {
        let pb = NSPasteboard.general
        guard pb.changeCount != lastChangeCount else { return }
        lastChangeCount = pb.changeCount
        // Stay well below the 32 MiB frame limit.
        guard let text = pb.string(forType: .string), text.utf8.count <= 16 << 20 else { return }
        send(Clipboard(mime: Self.mime, data: Data(text.utf8)).frame)
    }
}
