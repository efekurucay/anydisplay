import AppKit
import CoreGraphics
import AnyDisplayProtocol
import Foundation

/// Sends CURSOR_POS / CURSOR_SHAPE (PROTOCOL.md 5.4) when the client draws the cursor itself.
/// Polls on the main queue: position at the stream rate, shape ten times a second.
final class CursorTracker {
    private let displayID: CGDirectDisplayID
    private let streamWidth: Int
    private let streamHeight: Int
    private let fps: Int
    private let send: (Frame) -> Void
    private var posTimer: Timer?
    private var shapeTimer: Timer?
    private var lastPos: CursorPos?
    private var lastShapeKey: String?

    init(displayID: CGDirectDisplayID, streamWidth: Int, streamHeight: Int, fps: Int, send: @escaping (Frame) -> Void) {
        self.displayID = displayID
        self.streamWidth = streamWidth
        self.streamHeight = streamHeight
        self.fps = fps
        self.send = send
    }

    func start() {
        DispatchQueue.main.async {
            self.posTimer = Timer.scheduledTimer(withTimeInterval: 1 / Double(max(self.fps, 1)), repeats: true) { [weak self] _ in
                self?.pollPosition()
            }
            self.shapeTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
                self?.pollShape()
            }
            self.pollShape()
            self.pollPosition()
        }
    }

    func stop() {
        DispatchQueue.main.async {
            self.posTimer?.invalidate()
            self.shapeTimer?.invalidate()
            self.posTimer = nil
            self.shapeTimer = nil
        }
    }

    private var scale: CGFloat {
        let b = CGDisplayBounds(displayID)
        return b.width > 0 ? CGFloat(streamWidth) / b.width : 1
    }

    private func pollPosition() {
        guard let loc = CGEvent(source: nil)?.location else { return }
        let b = CGDisplayBounds(displayID)
        guard b.width > 0, b.height > 0 else { return }
        let inside = b.contains(loc)
        let x = (loc.x - b.minX) / b.width * CGFloat(streamWidth)
        let y = (loc.y - b.minY) / b.height * CGFloat(streamHeight)
        let pos = CursorPos(x: Int16(clamping: Int(x.rounded())), y: Int16(clamping: Int(y.rounded())), visible: inside)
        guard pos != lastPos else { return }
        // While outside, one "hidden" update is enough.
        if !inside, lastPos?.visible == 0 { return }
        lastPos = pos
        send(pos.frame)
    }

    private func pollShape() {
        guard let cursor = NSCursor.currentSystem else { return }
        let image = cursor.image
        let s = scale
        let w = Int((image.size.width * s).rounded()), h = Int((image.size.height * s).rounded())
        guard w > 0, h > 0, w <= 256, h <= 256 else { return }
        let hot = cursor.hotSpot
        guard let rgba = Self.rgba(image, width: w, height: h) else { return }
        let key = "\(w)x\(h)@\(hot.x),\(hot.y)#\(rgba.hashValue)"
        guard key != lastShapeKey else { return }
        lastShapeKey = key
        send(CursorShape(w: UInt16(w), h: UInt16(h), hotX: UInt16(min(max(hot.x * s, 0), CGFloat(w - 1))),
                         hotY: UInt16(min(max(hot.y * s, 0), CGFloat(h - 1))), rgba: rgba).frame)
    }

    /// Straight-alpha RGBA8 pixels of `image` rendered at `width x height`.
    private static func rgba(_ image: NSImage, width: Int, height: Int) -> Data? {
        var rect = CGRect(x: 0, y: 0, width: width, height: height)
        guard let cg = image.cgImage(forProposedRect: &rect, context: nil, hints: nil),
              let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: width, height: height))
        guard let ptr = ctx.data else { return nil }
        var out = Data(bytes: ptr, count: width * height * 4)
        out.withUnsafeMutableBytes { raw in
            let p = raw.bindMemory(to: UInt8.self)
            for i in stride(from: 0, to: p.count, by: 4) {
                let a = Int(p[i + 3])
                guard a > 0, a < 255 else { continue }
                for c in 0..<3 { p[i + c] = UInt8(min(255, Int(p[i + c]) * 255 / a)) }
            }
        }
        return out
    }
}
