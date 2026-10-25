import AppKit
import CoreGraphics
import AnyDisplayProtocol
import Foundation

/// Turns client TOUCH / PEN / KEY / TEXT / SCROLL / MOUSE messages (PROTOCOL.md 5.5) into CGEvents
/// on the streamed display. Pen samples become tablet events (pressure, tilt, rotation, proximity);
/// one finger drives the left button, two fingers scroll, a two-finger tap right-clicks.
/// Requires the Accessibility permission. Runs on its own queue.
public final class InputInjector {
    public struct Target {
        public var displayID: CGDirectDisplayID
        public var streamWidth: Int
        public var streamHeight: Int
    }

    private let queue = DispatchQueue(label: "anydisplay.input", qos: .userInteractive)
    private let source = CGEventSource(stateID: .hidSystemState)
    private var target: Target?

    // Mouse / button state
    private var location = CGPoint.zero
    private var buttonsDown = Set<CGMouseButton>()
    private var lastClick: (time: TimeInterval, location: CGPoint, button: CGMouseButton, count: Int)?

    // Touch gesture state
    private enum TouchMode { case idle, pending(id: UInt8), pointer(id: UInt8), scroll, finished }
    private var touchMode = TouchMode.idle
    private var touchStart = CGPoint.zero
    private var touchStartTime: TimeInterval = 0
    private var pendingTimer: DispatchWorkItem?
    private var scrollCentroid = CGPoint.zero
    private var scrollMoved = false
    private var maxContacts = 0
    private static let touchSlop: CGFloat = 6        // points
    private static let pressDelay: TimeInterval = 0.12
    private static let tapTime: TimeInterval = 0.3

    // Pen state
    private var penInProximity = false
    private var penDown = false
    private var penButton = CGMouseButton.left
    private var penTool: UInt8 = 0
    private static let penDeviceID: Int64 = 0x45_4B

    public init() {}

    public func setTarget(_ t: Target?) {
        queue.async { self.target = t }
    }

    public func handle(_ frame: Frame) {
        queue.async { self.dispatch(frame) }
    }

    /// Releases anything still pressed (session end).
    public func reset() {
        queue.async {
            for b in self.buttonsDown { self.post(self.mouseEvent(self.upType(b), b, self.location)) }
            self.buttonsDown.removeAll()
            if self.penInProximity { self.postProximity(enter: false) }
            self.penInProximity = false
            self.penDown = false
            self.pendingTimer?.cancel()
            self.touchMode = .idle
        }
    }

    private func dispatch(_ frame: Frame) {
        guard target != nil else { return }
        do {
            switch frame.type {
            case MessageType.touch: onTouch(try Touch(body: frame.body))
            case MessageType.pen: onPen(try Pen(body: frame.body))
            case MessageType.key: onKey(try Key(body: frame.body))
            case MessageType.text: onText(try Text(body: frame.body))
            case MessageType.scroll: onScroll(try Scroll(body: frame.body))
            case MessageType.mouse: onMouse(try Mouse(body: frame.body))
            default: break
            }
        } catch {
            Log.debug("bad input message 0x\(String(frame.type, radix: 16)): \(error)")
        }
    }

    // MARK: - Coordinates

    private var bounds: CGRect {
        guard let t = target else { return .zero }
        return CGDisplayBounds(t.displayID)
    }

    /// u16 normalized (0 = left/top, 65535 = right/bottom) to global display points.
    private func point(nx: UInt16, ny: UInt16) -> CGPoint {
        let b = bounds
        let x = b.minX + CGFloat(nx) / 65535 * b.width
        let y = b.minY + CGFloat(ny) / 65535 * b.height
        return CGPoint(x: min(x, b.maxX - 0.5), y: min(y, b.maxY - 0.5))
    }

    /// Stream pixels to global display points.
    private func point(sx: UInt16, sy: UInt16) -> CGPoint {
        guard let t = target, t.streamWidth > 0, t.streamHeight > 0 else { return .zero }
        let b = bounds
        let x = b.minX + CGFloat(sx) / CGFloat(t.streamWidth) * b.width
        let y = b.minY + CGFloat(sy) / CGFloat(t.streamHeight) * b.height
        return CGPoint(x: min(x, b.maxX - 0.5), y: min(y, b.maxY - 0.5))
    }

    private var pointsPerStreamPixel: CGSize {
        guard let t = target, t.streamWidth > 0, t.streamHeight > 0 else { return CGSize(width: 1, height: 1) }
        let b = bounds
        return CGSize(width: b.width / CGFloat(t.streamWidth), height: b.height / CGFloat(t.streamHeight))
    }

    // MARK: - Mouse primitives

    private func downType(_ b: CGMouseButton) -> CGEventType {
        switch b { case .left: return .leftMouseDown; case .right: return .rightMouseDown; default: return .otherMouseDown }
    }

    private func upType(_ b: CGMouseButton) -> CGEventType {
        switch b { case .left: return .leftMouseUp; case .right: return .rightMouseUp; default: return .otherMouseUp }
    }

    private func dragType(_ b: CGMouseButton) -> CGEventType {
        switch b { case .left: return .leftMouseDragged; case .right: return .rightMouseDragged; default: return .otherMouseDragged }
    }

    private func mouseEvent(_ type: CGEventType, _ button: CGMouseButton, _ p: CGPoint) -> CGEvent? {
        CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: p, mouseButton: button)
    }

    private func post(_ e: CGEvent?) {
        e?.post(tap: .cghidEventTap)
    }

    private func move(to p: CGPoint) {
        location = p
        if let b = buttonsDown.first {
            post(mouseEvent(dragType(b), b, p))
        } else {
            post(mouseEvent(.mouseMoved, .left, p))
        }
    }

    private func clickCount(_ button: CGMouseButton, _ p: CGPoint) -> Int {
        let now = ProcessInfo.processInfo.systemUptime
        var count = 1
        if let c = lastClick, c.button == button, now - c.time < NSEvent.doubleClickInterval,
           abs(c.location.x - p.x) < 4, abs(c.location.y - p.y) < 4 {
            count = c.count + 1
        }
        lastClick = (now, p, button, count)
        return count
    }

    private func press(_ button: CGMouseButton, at p: CGPoint) {
        location = p
        buttonsDown.insert(button)
        let e = mouseEvent(downType(button), button, p)
        e?.setIntegerValueField(.mouseEventClickState, value: Int64(clickCount(button, p)))
        post(e)
    }

    private func release(_ button: CGMouseButton, at p: CGPoint) {
        guard buttonsDown.contains(button) else { return }
        location = p
        buttonsDown.remove(button)
        let e = mouseEvent(upType(button), button, p)
        e?.setIntegerValueField(.mouseEventClickState, value: Int64(lastClick?.count ?? 1))
        post(e)
    }

    // MARK: - MOUSE

    private func onMouse(_ m: Mouse) {
        let p = point(nx: m.x, ny: m.y)
        let button: CGMouseButton = m.button == 1 ? .right : m.button == 2 ? .center : .left
        switch m.action {
        case 0: move(to: p)
        case 1: press(button, at: p)
        case 2: release(button, at: p)
        default: break
        }
    }

    // MARK: - TOUCH

    private func onTouch(_ t: Touch) {
        let active = t.contacts.filter { $0.action == 0 || $0.action == 1 }
        let cancelled = t.contacts.contains { $0.action == 3 }
        maxContacts = max(maxContacts, active.count)
        let now = ProcessInfo.processInfo.systemUptime

        if cancelled {
            endTouch(cancel: true)
            return
        }

        switch touchMode {
        case .idle:
            guard let first = active.first else { return }
            maxContacts = active.count
            touchStartTime = now
            if active.count >= 2 {
                beginScroll(active)
            } else {
                touchStart = point(nx: first.x, ny: first.y)
                move(to: touchStart)
                touchMode = .pending(id: first.id)
                let id = first.id
                let work = DispatchWorkItem { [weak self] in
                    guard let self, case .pending(let pid) = self.touchMode, pid == id else { return }
                    self.press(.left, at: self.location)
                    self.touchMode = .pointer(id: id)
                }
                pendingTimer = work
                queue.asyncAfter(deadline: .now() + Self.pressDelay, execute: work)
            }

        case .pending(let id):
            if active.count >= 2 {
                pendingTimer?.cancel()
                beginScroll(active)
            } else if let c = active.first(where: { $0.id == id }) {
                let p = point(nx: c.x, ny: c.y)
                location = p
                if hypot(p.x - touchStart.x, p.y - touchStart.y) > Self.touchSlop {
                    pendingTimer?.cancel()
                    press(.left, at: touchStart)
                    move(to: p)
                    touchMode = .pointer(id: id)
                }
            } else {
                // Lifted before the press delay: a tap.
                pendingTimer?.cancel()
                press(.left, at: location)
                release(.left, at: location)
                touchMode = .idle
            }

        case .pointer(let id):
            if let c = active.first(where: { $0.id == id }) {
                move(to: point(nx: c.x, ny: c.y))
            } else {
                release(.left, at: location)
                touchMode = active.isEmpty ? .idle : .finished
            }

        case .scroll:
            if active.isEmpty {
                endTouch(cancel: false)
            } else {
                let c = centroid(active)
                let d = CGPoint(x: c.x - scrollCentroid.x, y: c.y - scrollCentroid.y)
                if hypot(c.x - touchStart.x, c.y - touchStart.y) > Self.touchSlop { scrollMoved = true }
                scrollCentroid = c
                if d.x != 0 || d.y != 0 { postScroll(dx: d.x, dy: d.y, phase: 2, momentum: 0, at: c) }
            }

        case .finished:
            // Remaining fingers of a gesture that already ended; wait for all to lift.
            if active.isEmpty { touchMode = .idle }
        }
    }

    private func centroid(_ contacts: [TouchContact]) -> CGPoint {
        let pts = contacts.map { point(nx: $0.x, ny: $0.y) }
        let n = CGFloat(pts.count)
        return CGPoint(x: pts.map(\.x).reduce(0, +) / n, y: pts.map(\.y).reduce(0, +) / n)
    }

    private func beginScroll(_ active: [TouchContact]) {
        touchMode = .scroll
        scrollCentroid = centroid(active)
        touchStart = scrollCentroid
        scrollMoved = false
        move(to: scrollCentroid)
        postScroll(dx: 0, dy: 0, phase: 1, momentum: 0, at: scrollCentroid)
    }

    private func endTouch(cancel: Bool) {
        pendingTimer?.cancel()
        switch touchMode {
        case .pointer, .pending:
            release(.left, at: location)
        case .scroll:
            postScroll(dx: 0, dy: 0, phase: 4, momentum: 0, at: scrollCentroid)
            let quick = ProcessInfo.processInfo.systemUptime - touchStartTime < Self.tapTime
            if !cancel, !scrollMoved, quick, maxContacts == 2 {
                press(.right, at: scrollCentroid)
                release(.right, at: scrollCentroid)
            }
        default:
            break
        }
        touchMode = .idle
    }

    // MARK: - SCROLL

    private func onScroll(_ s: Scroll) {
        let p = point(sx: s.x, sy: s.y)
        if hypot(p.x - location.x, p.y - location.y) > 0.5 { move(to: p) }
        let k = pointsPerStreamPixel
        let dx = CGFloat(s.dx) * k.width, dy = CGFloat(s.dy) * k.height
        switch s.phase {
        case 0: postScroll(dx: dx, dy: dy, phase: 1, momentum: 0, at: p)
        case 1: postScroll(dx: dx, dy: dy, phase: 2, momentum: 0, at: p)
        case 2: postScroll(dx: dx, dy: dy, phase: 4, momentum: 0, at: p)
        case 3: postScroll(dx: dx, dy: dy, phase: 0, momentum: 2, at: p)
        default: break
        }
    }

    /// Positive dx/dy move the content right/down, which is what positive wheel deltas do in CGEvents.
    /// - Parameters:
    ///   - phase: CGScrollPhase (1 began, 2 changed, 4 ended, 0 none).
    ///   - momentum: CGMomentumScrollPhase (0 none, 2 continue).
    private func postScroll(dx: CGFloat, dy: CGFloat, phase: Int64, momentum: Int64, at p: CGPoint) {
        guard let e = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2,
                              wheel1: Int32(dy.rounded()), wheel2: Int32(dx.rounded()), wheel3: 0) else { return }
        e.location = p
        e.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
        e.setIntegerValueField(.scrollWheelEventScrollPhase, value: phase)
        e.setIntegerValueField(.scrollWheelEventMomentumPhase, value: momentum)
        e.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1, value: Double(dy))
        e.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis2, value: Double(dx))
        e.setDoubleValueField(.scrollWheelEventPointDeltaAxis1, value: Double(dy))
        e.setDoubleValueField(.scrollWheelEventPointDeltaAxis2, value: Double(dx))
        post(e)
    }

    // MARK: - PEN

    private func onPen(_ pen: Pen) {
        for s in pen.samples { onPenSample(s) }
    }

    private func onPenSample(_ s: PenSample) {
        let p = point(nx: s.x, ny: s.y)
        if s.tool != penTool, penInProximity, !penDown {
            // Switching between tip and eraser is a new proximity session for macOS apps.
            postProximity(enter: false)
            penInProximity = false
        }
        penTool = s.tool
        switch s.action {
        case 3:  // hover_enter
            if !penInProximity { postProximity(enter: true) }
            penInProximity = true
            postPen(.mouseMoved, .left, p, s, pressure: 0)
        case 4:  // hover_move
            if !penInProximity { postProximity(enter: true); penInProximity = true }
            postPen(.mouseMoved, .left, p, s, pressure: 0)
        case 5:  // hover_exit
            if penInProximity { postProximity(enter: false) }
            penInProximity = false
        case 0:  // down
            if !penInProximity { postProximity(enter: true); penInProximity = true }
            penButton = s.buttons & 1 != 0 ? .right : .left
            penDown = true
            buttonsDown.insert(penButton)
            location = p
            let e = penEvent(downType(penButton), penButton, p, s, pressure: s.pressure)
            e?.setIntegerValueField(.mouseEventClickState, value: Int64(clickCount(penButton, p)))
            post(e)
        case 1:  // move
            if penDown {
                location = p
                postPen(dragType(penButton), penButton, p, s, pressure: s.pressure)
            } else {
                postPen(.mouseMoved, .left, p, s, pressure: 0)
            }
        case 2, 6:  // up, cancel
            if penDown {
                penDown = false
                buttonsDown.remove(penButton)
                location = p
                let e = penEvent(upType(penButton), penButton, p, s, pressure: 0)
                e?.setIntegerValueField(.mouseEventClickState, value: Int64(lastClick?.count ?? 1))
                post(e)
            }
            if s.action == 6, penInProximity {
                postProximity(enter: false)
                penInProximity = false
            }
        default:
            break
        }
    }

    private func postPen(_ type: CGEventType, _ button: CGMouseButton, _ p: CGPoint, _ s: PenSample, pressure: UInt16) {
        if type == .mouseMoved { location = p }
        post(penEvent(type, button, p, s, pressure: pressure))
    }

    private func penEvent(_ type: CGEventType, _ button: CGMouseButton, _ p: CGPoint, _ s: PenSample,
                          pressure: UInt16) -> CGEvent? {
        guard let e = mouseEvent(type, button, p) else { return nil }
        let pr = Double(pressure) / 65535
        e.setIntegerValueField(.mouseEventSubtype, value: Int64(CGEventMouseSubtype.tabletPoint.rawValue))
        e.setDoubleValueField(.mouseEventPressure, value: pr)
        e.setIntegerValueField(.tabletEventPointX, value: Int64(s.x))
        e.setIntegerValueField(.tabletEventPointY, value: Int64(s.y))
        e.setIntegerValueField(.tabletEventPointButtons, value: Int64(pressure > 0 ? 1 : 0) | Int64(s.buttons) << 1)
        e.setDoubleValueField(.tabletEventPointPressure, value: pr)
        e.setDoubleValueField(.tabletEventTiltX, value: Double(s.tiltX) / 9000)
        e.setDoubleValueField(.tabletEventTiltY, value: Double(s.tiltY) / 9000)
        e.setDoubleValueField(.tabletEventRotation, value: Double(s.twist) / 100)
        e.setIntegerValueField(.tabletEventDeviceID, value: Self.penDeviceID)
        return e
    }

    private func postProximity(enter: Bool) {
        guard let e = CGEvent(source: source) else { return }
        e.type = .tabletProximity
        e.location = location
        e.setIntegerValueField(.mouseEventSubtype, value: Int64(CGEventMouseSubtype.tabletProximity.rawValue))
        e.setIntegerValueField(.tabletProximityEventVendorID, value: 0x056A)
        e.setIntegerValueField(.tabletProximityEventTabletID, value: 1)
        e.setIntegerValueField(.tabletProximityEventPointerID, value: penTool == 1 ? 2 : 1)
        e.setIntegerValueField(.tabletProximityEventDeviceID, value: Self.penDeviceID)
        e.setIntegerValueField(.tabletProximityEventSystemTabletID, value: 1)
        e.setIntegerValueField(.tabletProximityEventVendorPointerType, value: penTool == 1 ? 0x080A : 0x0802)
        e.setIntegerValueField(.tabletProximityEventVendorPointerSerialNumber, value: 1)
        // NX_TABLET_CAPABILITY_*: device id, abs x, abs y, buttons, tilt x, tilt y, pressure, rotation.
        e.setIntegerValueField(.tabletProximityEventCapabilityMask,
                               value: 0x0001 | 0x0002 | 0x0004 | 0x0040 | 0x0080 | 0x0100 | 0x0400 | 0x2000)
        e.setIntegerValueField(.tabletProximityEventPointerType, value: penTool == 1 ? 3 : 1)  // NX_TABLET_POINTER_ERASER / PEN
        e.setIntegerValueField(.tabletProximityEventEnterProximity, value: enter ? 1 : 0)
        post(e)
    }

    // MARK: - KEY / TEXT

    private func flags(_ modifiers: UInt16) -> CGEventFlags {
        var f = CGEventFlags()
        if modifiers & 1 != 0 { f.insert(.maskControl) }
        if modifiers & 2 != 0 { f.insert(.maskShift) }
        if modifiers & 4 != 0 { f.insert(.maskAlternate) }
        if modifiers & 8 != 0 { f.insert(.maskCommand) }
        return f
    }

    private func onKey(_ k: Key) {
        guard let code = KeyMap.keyCode(hidUsage: k.hidUsage),
              let e = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: k.action == 1) else {
            Log.debug("unmapped HID usage 0x\(String(k.hidUsage, radix: 16))")
            return
        }
        var f = flags(k.modifiers)
        // Arrow and navigation keys carry the function / numeric-pad flags on a Mac keyboard.
        if (0x4A...0x52).contains(k.hidUsage) { f.insert(.maskSecondaryFn) }
        if (0x4F...0x52).contains(k.hidUsage) { f.insert(.maskNumericPad) }
        e.flags = f
        post(e)
    }

    private func onText(_ t: Text) {
        let units = Array(t.text.utf16)
        var i = 0
        while i < units.count {
            let chunk = Array(units[i..<min(i + 20, units.count)])
            i += chunk.count
            for down in [true, false] {
                guard let e = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down) else { continue }
                e.flags = []
                e.keyboardSetUnicodeString(stringLength: chunk.count, unicodeString: chunk)
                post(e)
            }
        }
    }
}
