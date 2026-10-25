import CoreGraphics
import CVirtualDisplay
import Foundation

/// An extra desktop created with CoreGraphics' CGVirtualDisplay. It exists while this object lives.
///
/// macOS derives a list of modes from the one we register (scaled variants, HiDPI variants when
/// `maxPixels` allows 2x) and starts the display in an arbitrary one, so `apply` selects the
/// wanted mode explicitly.
public final class VirtualDisplay {
    public let displayID: CGDirectDisplayID
    private let display: CGVirtualDisplay
    public let maxPixels: Int

    /// - Parameter maxPixels: the largest pixel edge later `apply` calls may use (covers rotation).
    public init?(name: String, serial: UInt32, sizeMillimeters: CGSize, maxPixels: Int) {
        let desc = CGVirtualDisplayDescriptor()
        desc.queue = DispatchQueue(label: "anydisplay.virtual-display")
        desc.name = name
        desc.maxPixelsWide = UInt32(maxPixels)
        desc.maxPixelsHigh = UInt32(maxPixels)
        desc.sizeInMillimeters = sizeMillimeters
        desc.vendorID = 0xE4A1
        desc.productID = 0x0001
        desc.serialNum = serial
        // sRGB primaries and D65 white point.
        desc.redPrimary = CGPoint(x: 0.64, y: 0.33)
        desc.greenPrimary = CGPoint(x: 0.30, y: 0.60)
        desc.bluePrimary = CGPoint(x: 0.15, y: 0.06)
        desc.whitePoint = CGPoint(x: 0.3127, y: 0.3290)
        desc.terminationHandler = { _, _ in Log.info("virtual display terminated by the system") }
        guard let d = CGVirtualDisplay(descriptor: desc), d.displayID != 0 else { return nil }
        display = d
        displayID = d.displayID
        self.maxPixels = maxPixels
    }

    /// Registers a single mode of `pixelWidth x pixelHeight` backing pixels (`/2` points when `hidpi`)
    /// and switches the display to it. Returns false if CoreGraphics does not offer that mode.
    public func apply(pixelWidth: Int, pixelHeight: Int, hidpi: Bool, refreshHz: Double) -> Bool {
        guard max(pixelWidth, pixelHeight) <= maxPixels else { return false }
        let scale = hidpi ? 2 : 1
        let pointsW = pixelWidth / scale, pointsH = pixelHeight / scale
        let settings = CGVirtualDisplaySettings()
        settings.hiDPI = hidpi ? 1 : 0
        settings.modes = [CGVirtualDisplayMode(width: UInt(pointsW), height: UInt(pointsH), refreshRate: refreshHz)]
        guard display.apply(settings) else { return false }
        return select(pointsW: pointsW, pointsH: pointsH, pixelW: pixelWidth, pixelH: pixelHeight)
    }

    /// Current desktop size in points and pixels, once the display is online.
    public var currentMode: (points: CGSize, pixels: CGSize)? {
        guard let mode = CGDisplayCopyDisplayMode(displayID) else { return nil }
        return (CGSize(width: mode.width, height: mode.height), CGSize(width: mode.pixelWidth, height: mode.pixelHeight))
    }

    private func matches(_ m: CGDisplayMode, _ pw: Int, _ ph: Int, _ xw: Int, _ xh: Int) -> Bool {
        m.width == pw && m.height == ph && m.pixelWidth == xw && m.pixelHeight == xh
    }

    private func select(pointsW: Int, pointsH: Int, pixelW: Int, pixelH: Int) -> Bool {
        let opts = [kCGDisplayShowDuplicateLowResolutionModes: true] as CFDictionary
        let deadline = Date().addingTimeInterval(3)
        var target: CGDisplayMode?
        while Date() < deadline {
            if let cur = CGDisplayCopyDisplayMode(displayID), matches(cur, pointsW, pointsH, pixelW, pixelH) { return true }
            let modes = (CGDisplayCopyAllDisplayModes(displayID, opts) as? [CGDisplayMode]) ?? []
            target = modes.first { $0.isUsableForDesktopGUI() && matches($0, pointsW, pointsH, pixelW, pixelH) }
            if target != nil { break }
            Thread.sleep(forTimeInterval: 0.05)
        }
        guard let target else {
            let offered = ((CGDisplayCopyAllDisplayModes(displayID, opts) as? [CGDisplayMode]) ?? [])
                .filter { $0.isUsableForDesktopGUI() }.map { "\($0.width)x\($0.height)@\($0.pixelWidth)x\($0.pixelHeight)" }
            Log.info("virtual display \(displayID): mode \(pointsW)x\(pointsH)@\(pixelW)x\(pixelH) not offered; online=\(CGDisplayIsOnline(displayID)) offered=\(offered)")
            return false
        }
        var config: CGDisplayConfigRef?
        guard CGBeginDisplayConfiguration(&config) == .success else { return false }
        CGConfigureDisplayWithDisplayMode(config, displayID, target, nil)
        guard CGCompleteDisplayConfiguration(config, .forSession) == .success else { return false }
        while Date() < deadline {
            if let cur = CGDisplayCopyDisplayMode(displayID), matches(cur, pointsW, pointsH, pixelW, pixelH) { return true }
            Thread.sleep(forTimeInterval: 0.05)
        }
        return false
    }
}
