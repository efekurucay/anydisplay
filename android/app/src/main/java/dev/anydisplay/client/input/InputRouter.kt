package dev.anydisplay.client.input

import android.graphics.RectF
import android.os.SystemClock
import android.view.MotionEvent
import android.view.View
import dev.anydisplay.client.data.TouchMode
import dev.anydisplay.client.protocol.Bodies
import dev.anydisplay.client.protocol.MouseAction
import dev.anydisplay.client.protocol.MouseButton
import dev.anydisplay.client.protocol.PenAction
import dev.anydisplay.client.protocol.PenSample
import dev.anydisplay.client.protocol.Proto
import dev.anydisplay.client.protocol.ScrollPhase
import dev.anydisplay.client.protocol.TouchAction
import dev.anydisplay.client.protocol.TouchContact
import dev.anydisplay.client.protocol.encodePen
import dev.anydisplay.client.protocol.encodeTouch
import dev.anydisplay.client.protocol.normalize

/**
 * Converts Android MotionEvents over the video into PROTOCOL.md 5.5 input:
 * stylus/eraser -> PEN (with history, hover, tilt, buttons), fingers -> TOUCH (direct) or MOUSE/SCROLL (trackpad),
 * mouse -> MOUSE/SCROLL. Palm rejection drops fingers while a stylus is in range and for [PALM_HOLD_MS] after.
 */
class InputRouter(
    private val view: View,
    private val rect: RectF,
    private val send: (type: Int, body: ByteArray) -> Unit,
    touchSlopPx: Float,
) : TrackpadEngine.Sink {
    var mode = TouchMode.DIRECT
        set(v) {
            if (v != field) cancelFingers()
            field = v
        }
    var palmRejection = true
    var streamWidth = 1
    var streamHeight = 1

    private val engine = TrackpadEngine(this, touchSlopPx)
    private var stylusInRange = false
    private var stylusLastMs = -10_000L
    private var rejectingFingers = false
    private val reported = LinkedHashMap<Int, IntArray>() // finger id -> last (x, y, pressure, size)
    private val lastPos = HashMap<Int, Pair<Float, Float>>()
    private var mouseButtons = 0

    val trackpadCursor get() = engine.cursorX to engine.cursorY

    fun onGeometryChanged() {
        engine.setGeometry(rect.width(), rect.height(), streamWidth, streamHeight)
    }

    /** Host cursor position (normalized) from CURSOR_POS keeps the virtual trackpad cursor in sync. */
    fun syncCursor(nx: Float, ny: Float) = engine.setCursor(nx, ny)

    private fun nx(x: Float) = normalize(x, rect.left, rect.width())
    private fun ny(y: Float) = normalize(y, rect.top, rect.height())

    private fun isPen(tool: Int) = tool == MotionEvent.TOOL_TYPE_STYLUS || tool == MotionEvent.TOOL_TYPE_ERASER

    fun onTouchEvent(ev: MotionEvent): Boolean {
        val tool = ev.getToolType(ev.actionIndex)
        val penIndex = (0 until ev.pointerCount).firstOrNull { isPen(ev.getToolType(it)) }
        when {
            penIndex != null -> handlePen(ev, if (isPen(tool)) ev.actionIndex else penIndex)
            tool == MotionEvent.TOOL_TYPE_MOUSE -> handleMouse(ev)
            else -> handleFingers(ev)
        }
        return true
    }

    fun onGenericMotionEvent(ev: MotionEvent): Boolean {
        val tool = ev.getToolType(0)
        return when {
            isPen(tool) -> { handlePen(ev, 0); true }
            tool == MotionEvent.TOOL_TYPE_MOUSE || ev.isFromSource(android.view.InputDevice.SOURCE_MOUSE) -> { handleMouse(ev); true }
            else -> false
        }
    }

    // ------------------------------------------------------------ pen

    private fun handlePen(ev: MotionEvent, index: Int) {
        val action = when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> PenAction.DOWN
            MotionEvent.ACTION_MOVE -> PenAction.MOVE
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> PenAction.UP
            MotionEvent.ACTION_CANCEL -> PenAction.CANCEL
            MotionEvent.ACTION_HOVER_ENTER -> PenAction.HOVER_ENTER
            MotionEvent.ACTION_HOVER_MOVE -> PenAction.HOVER_MOVE
            MotionEvent.ACTION_HOVER_EXIT -> PenAction.HOVER_EXIT
            else -> return
        }
        stylusLastMs = SystemClock.uptimeMillis()
        stylusInRange = action != PenAction.HOVER_EXIT && action != PenAction.CANCEL && action != PenAction.UP
        if (palmRejection) cancelFingers()

        val tool = if (ev.getToolType(index) == MotionEvent.TOOL_TYPE_ERASER) 1 else 0
        val bs = ev.buttonState
        var buttons = 0
        if (bs and (MotionEvent.BUTTON_STYLUS_PRIMARY or MotionEvent.BUTTON_SECONDARY) != 0) buttons = buttons or 1
        if (bs and (MotionEvent.BUTTON_STYLUS_SECONDARY or MotionEvent.BUTTON_TERTIARY) != 0) buttons = buttons or 2
        val hovering = action >= PenAction.HOVER_ENTER && action <= PenAction.HOVER_EXIT
        val samples = ArrayList<PenSample>(ev.historySize + 1)
        if (action == PenAction.MOVE || action == PenAction.HOVER_MOVE) {
            for (h in 0 until ev.historySize) {
                samples += penSample(action, tool, buttons, hovering,
                    ev.getHistoricalX(index, h), ev.getHistoricalY(index, h), ev.getHistoricalPressure(index, h),
                    ev.getHistoricalAxisValue(MotionEvent.AXIS_TILT, index, h),
                    ev.getHistoricalAxisValue(MotionEvent.AXIS_ORIENTATION, index, h), ev.getHistoricalEventTime(h))
            }
        }
        samples += penSample(action, tool, buttons, hovering, ev.getX(index), ev.getY(index), ev.getPressure(index),
            ev.getAxisValue(MotionEvent.AXIS_TILT, index), ev.getAxisValue(MotionEvent.AXIS_ORIENTATION, index), ev.eventTime)
        for (body in encodePen(samples)) send(Proto.PEN, body)
    }

    private fun penSample(
        action: Int, tool: Int, buttons: Int, hovering: Boolean,
        x: Float, y: Float, pressure: Float, tilt: Float, orientation: Float, timeMs: Long,
    ): PenSample {
        val (tx, ty) = PenMath.tiltCentidegrees(tilt, orientation)
        val p = if (hovering || action == PenAction.UP) 0 else (pressure.coerceIn(0f, 1f) * 65535f).toInt()
        return PenSample(action, tool, buttons, nx(x), ny(y), p, tx, ty, 0, timeMs)
    }

    // ------------------------------------------------------------ fingers

    private fun palmBlocked(): Boolean =
        palmRejection && (stylusInRange || SystemClock.uptimeMillis() - stylusLastMs < PALM_HOLD_MS)

    private fun handleFingers(ev: MotionEvent) {
        val masked = ev.actionMasked
        if (mode == TouchMode.PEN_ONLY) return
        if (masked == MotionEvent.ACTION_DOWN) rejectingFingers = palmBlocked()
        if (!rejectingFingers && palmBlocked()) {
            cancelFingers()
            rejectingFingers = true
        }
        if (rejectingFingers) {
            if (masked == MotionEvent.ACTION_UP || masked == MotionEvent.ACTION_CANCEL) rejectingFingers = false
            return
        }
        when (mode) {
            TouchMode.DIRECT -> directTouch(ev)
            TouchMode.TRACKPAD -> trackpad(ev)
            TouchMode.PEN_ONLY -> Unit
        }
    }

    private fun contact(ev: MotionEvent, i: Int, action: Int, h: Int = -1): TouchContact {
        val x = if (h < 0) ev.getX(i) else ev.getHistoricalX(i, h)
        val y = if (h < 0) ev.getY(i) else ev.getHistoricalY(i, h)
        val p = if (h < 0) ev.getPressure(i) else ev.getHistoricalPressure(i, h)
        val s = if (h < 0) ev.getSize(i) else ev.getHistoricalSize(i, h)
        val id = ev.getPointerId(i) and 0xFF
        val c = TouchContact(id, action, nx(x), ny(y), (p.coerceIn(0f, 1f) * 65535f).toInt(), (s.coerceIn(0f, 1f) * 65535f).toInt())
        if (action == TouchAction.UP || action == TouchAction.CANCEL) reported.remove(id)
        else reported[id] = intArrayOf(c.x, c.y, c.pressure, c.size)
        return c
    }

    private fun directTouch(ev: MotionEvent) {
        val masked = ev.actionMasked
        val fingers = (0 until ev.pointerCount).filter { !isPen(ev.getToolType(it)) }
        when (masked) {
            MotionEvent.ACTION_MOVE -> {
                for (h in 0 until ev.historySize) send(Proto.TOUCH, encodeTouch(fingers.map { contact(ev, it, TouchAction.MOVE, h) }))
                send(Proto.TOUCH, encodeTouch(fingers.map { contact(ev, it, TouchAction.MOVE) }))
            }
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN, MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> {
                val down = masked == MotionEvent.ACTION_DOWN || masked == MotionEvent.ACTION_POINTER_DOWN
                val changed = ev.actionIndex
                // Contacts that are still down first, then the one that changed state.
                val list = fingers.filter { it != changed }.map { contact(ev, it, TouchAction.MOVE) } +
                    contact(ev, changed, if (down) TouchAction.DOWN else TouchAction.UP)
                send(Proto.TOUCH, encodeTouch(list))
                if (masked == MotionEvent.ACTION_UP) reported.clear()
            }
            MotionEvent.ACTION_CANCEL -> cancelFingers()
        }
    }

    private fun trackpad(ev: MotionEvent) {
        val t = ev.eventTime
        val masked = ev.actionMasked
        fun lx(x: Float) = x - rect.left
        fun ly(y: Float) = y - rect.top
        when (masked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                val i = ev.actionIndex
                val id = ev.getPointerId(i)
                engine.down(id, lx(ev.getX(i)), ly(ev.getY(i)), t)
                lastPos[id] = lx(ev.getX(i)) to ly(ev.getY(i))
            }
            MotionEvent.ACTION_MOVE -> {
                for (h in 0..ev.historySize) {
                    val prev = HashMap(lastPos)
                    for (i in 0 until ev.pointerCount) {
                        val id = ev.getPointerId(i)
                        val x = lx(if (h < ev.historySize) ev.getHistoricalX(i, h) else ev.getX(i))
                        val y = ly(if (h < ev.historySize) ev.getHistoricalY(i, h) else ev.getY(i))
                        engine.move(id, x, y)
                        lastPos[id] = x to y
                    }
                    engine.commitMove(if (h < ev.historySize) ev.getHistoricalEventTime(h) else t, prev)
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> {
                val id = ev.getPointerId(ev.actionIndex)
                engine.up(id, t)
                lastPos.remove(id)
                if (engine.momentumActive) scheduleMomentum()
            }
            MotionEvent.ACTION_CANCEL -> {
                engine.cancel()
                lastPos.clear()
            }
        }
    }

    private val momentumTick = object : Runnable {
        override fun run() {
            if (engine.tick(SystemClock.uptimeMillis())) view.postOnAnimation(this)
        }
    }

    private fun scheduleMomentum() {
        view.removeCallbacks(momentumTick)
        view.postOnAnimation(momentumTick)
    }

    /** Lifts every finger the host knows about (stylus arrived, mode switch, window lost). */
    fun cancelFingers() {
        if (reported.isNotEmpty()) {
            val list = reported.map { (id, v) -> TouchContact(id, TouchAction.CANCEL, v[0], v[1], v[2], v[3]) }
            reported.clear()
            send(Proto.TOUCH, encodeTouch(list))
        }
        engine.cancel()
        lastPos.clear()
    }

    // ------------------------------------------------------------ mouse

    private fun handleMouse(ev: MotionEvent) {
        val x = nx(ev.x)
        val y = ny(ev.y)
        when (ev.actionMasked) {
            MotionEvent.ACTION_HOVER_MOVE, MotionEvent.ACTION_MOVE -> send(Proto.MOUSE, Bodies.mouse(MouseAction.MOVE, MouseButton.LEFT, x, y))
            MotionEvent.ACTION_BUTTON_PRESS, MotionEvent.ACTION_BUTTON_RELEASE, MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP -> {
                val now = ev.buttonState
                val changed = now xor mouseButtons
                for ((bit, button) in MOUSE_BUTTONS) {
                    if (changed and bit != 0) {
                        send(Proto.MOUSE, Bodies.mouse(if (now and bit != 0) MouseAction.DOWN else MouseAction.UP, button, x, y))
                    }
                }
                mouseButtons = now
            }
            MotionEvent.ACTION_SCROLL -> {
                val dx = (-ev.getAxisValue(MotionEvent.AXIS_HSCROLL) * WHEEL_PX).toInt()
                val dy = (ev.getAxisValue(MotionEvent.AXIS_VSCROLL) * WHEEL_PX).toInt()
                if (dx != 0 || dy != 0) send(Proto.SCROLL, Bodies.scroll(x, y, dx, dy, ScrollPhase.CHANGE))
            }
        }
    }

    // ------------------------------------------------------------ trackpad sink

    override fun mouse(action: Int, button: Int, nx: Float, ny: Float) =
        send(Proto.MOUSE, Bodies.mouse(action, button, toU16(nx), toU16(ny)))

    override fun scroll(nx: Float, ny: Float, dx: Int, dy: Int, phase: Int) =
        send(Proto.SCROLL, Bodies.scroll(toU16(nx), toU16(ny), dx, dy, phase))

    private fun toU16(n: Float) = (n.coerceIn(0f, 1f) * 65535f + 0.5f).toInt()

    companion object {
        const val PALM_HOLD_MS = 300L
        private const val WHEEL_PX = 48f
        private val MOUSE_BUTTONS = listOf(
            MotionEvent.BUTTON_PRIMARY to MouseButton.LEFT,
            MotionEvent.BUTTON_SECONDARY to MouseButton.RIGHT,
            MotionEvent.BUTTON_TERTIARY to MouseButton.MIDDLE,
        )
    }
}
