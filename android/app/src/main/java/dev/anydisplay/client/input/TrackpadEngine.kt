package dev.anydisplay.client.input

import dev.anydisplay.client.protocol.MouseAction
import dev.anydisplay.client.protocol.MouseButton
import dev.anydisplay.client.protocol.ScrollPhase
import kotlin.math.exp
import kotlin.math.hypot
import kotlin.math.roundToInt

/**
 * Turns finger gestures into relative-pointer input (pure logic, no Android types):
 * one-finger move = cursor move, tap = left click, tap then drag = left drag, two-finger tap = right click,
 * two-finger drag = scroll with begin/change/end phases and momentum.
 * Positions are view pixels inside the video rect; the engine keeps an absolute virtual cursor in [0,1].
 */
class TrackpadEngine(private val sink: Sink, private val touchSlopPx: Float = 12f) {
    interface Sink {
        fun mouse(action: Int, button: Int, nx: Float, ny: Float)
        fun scroll(nx: Float, ny: Float, dx: Int, dy: Int, phase: Int)
    }

    private class Finger(var x: Float, var y: Float, val startX: Float, val startY: Float)

    private val fingers = LinkedHashMap<Int, Finger>()
    private var rectW = 1f
    private var rectH = 1f
    private var streamW = 1f
    private var streamH = 1f

    var cursorX = 0.5f
        private set
    var cursorY = 0.5f
        private set

    private var gestureStartMs = 0L
    private var maxFingers = 0
    private var moved = false
    private var scrolling = false
    private var dragging = false
    private var pendingDrag = false
    private var frozen = false // after a scroll, the remaining finger must not move the cursor
    private var lastTapUpMs = -10_000L
    private var lastMoveMs = 0L

    private var velX = 0f // stream px per second, smoothed
    private var velY = 0f
    private var momentum = false
    private var momentumLastMs = 0L
    private var fracX = 0f
    private var fracY = 0f

    val momentumActive get() = momentum

    fun setGeometry(rectWidth: Float, rectHeight: Float, streamWidth: Int, streamHeight: Int) {
        rectW = rectWidth.coerceAtLeast(1f); rectH = rectHeight.coerceAtLeast(1f)
        streamW = streamWidth.coerceAtLeast(1).toFloat(); streamH = streamHeight.coerceAtLeast(1).toFloat()
    }

    fun down(id: Int, x: Float, y: Float, timeMs: Long) {
        if (fingers.isEmpty()) {
            stopMomentum()
            gestureStartMs = timeMs
            maxFingers = 0
            moved = false
            scrolling = false
            dragging = false
            frozen = false
            pendingDrag = timeMs - lastTapUpMs < DOUBLE_TAP_MS
        }
        fingers[id] = Finger(x, y, x, y)
        maxFingers = maxOf(maxFingers, fingers.size)
        if (fingers.size > 1) pendingDrag = false
        lastMoveMs = timeMs
    }

    fun move(id: Int, x: Float, y: Float) {
        fingers[id]?.let { it.x = x; it.y = y }
    }

    /** Call after updating all pointers of one MotionEvent sample with [move]. */
    fun commitMove(timeMs: Long, previous: Map<Int, Pair<Float, Float>>) {
        val dt = ((timeMs - lastMoveMs).coerceAtLeast(1)) / 1000f
        lastMoveMs = timeMs
        when {
            fingers.size == 1 && maxFingers == 1 && !frozen -> {
                val (id, f) = fingers.entries.first()
                val prev = previous[id] ?: return
                if (!moved && hypot(f.x - f.startX, f.y - f.startY) > touchSlopPx) {
                    moved = true
                    if (pendingDrag) {
                        dragging = true
                        sink.mouse(MouseAction.DOWN, MouseButton.LEFT, cursorX, cursorY)
                    }
                }
                if (!moved) return
                val dx = f.x - prev.first
                val dy = f.y - prev.second
                val speed = hypot(dx, dy) / (dt * 1000f) // px per ms
                val gain = BASE_GAIN * (1f + (speed.coerceAtMost(4f)) * ACCEL)
                cursorX = (cursorX + dx / rectW * gain).coerceIn(0f, 1f)
                cursorY = (cursorY + dy / rectH * gain).coerceIn(0f, 1f)
                sink.mouse(MouseAction.MOVE, MouseButton.LEFT, cursorX, cursorY)
            }
            fingers.size == 2 -> {
                var dx = 0f
                var dy = 0f
                var sx = 0f
                var sy = 0f
                for ((id, f) in fingers) {
                    val p = previous[id] ?: return
                    dx += (f.x - p.first) / 2f; dy += (f.y - p.second) / 2f
                    sx += (f.x - f.startX) / 2f; sy += (f.y - f.startY) / 2f
                }
                if (!moved && hypot(sx, sy) > touchSlopPx) {
                    moved = true
                    scrolling = true
                    velX = 0f; velY = 0f; fracX = 0f; fracY = 0f
                    sink.scroll(cursorX, cursorY, 0, 0, ScrollPhase.BEGIN)
                }
                if (!scrolling) return
                val sdx = dx * streamW / rectW
                val sdy = dy * streamH / rectH
                velX = 0.6f * velX + 0.4f * (sdx / dt)
                velY = 0.6f * velY + 0.4f * (sdy / dt)
                emitScroll(sdx, sdy, ScrollPhase.CHANGE)
            }
        }
    }

    fun up(id: Int, timeMs: Long) {
        if (fingers.remove(id) == null) return
        if (scrolling && fingers.size < 2) {
            scrolling = false
            frozen = true
            sink.scroll(cursorX, cursorY, 0, 0, ScrollPhase.END)
            // A finger that rests before lifting means no fling.
            if (timeMs - lastMoveMs < 80 && hypot(velX, velY) > MOMENTUM_MIN_START) {
                momentum = true
                momentumLastMs = timeMs
            }
        }
        if (fingers.isNotEmpty()) return
        val duration = timeMs - gestureStartMs
        when {
            dragging -> sink.mouse(MouseAction.UP, MouseButton.LEFT, cursorX, cursorY)
            !moved && duration < TAP_MS && maxFingers == 1 -> {
                click(MouseButton.LEFT)
                lastTapUpMs = timeMs
                return
            }
            !moved && duration < TAP_MS && maxFingers == 2 -> click(MouseButton.RIGHT)
        }
        lastTapUpMs = -10_000L
    }

    fun cancel() {
        if (dragging) sink.mouse(MouseAction.UP, MouseButton.LEFT, cursorX, cursorY)
        if (scrolling) sink.scroll(cursorX, cursorY, 0, 0, ScrollPhase.END)
        fingers.clear()
        scrolling = false
        dragging = false
        stopMomentum()
    }

    /** Advances scroll momentum; returns true while more ticks are needed. */
    fun tick(timeMs: Long): Boolean {
        if (!momentum) return false
        val dt = ((timeMs - momentumLastMs).coerceAtLeast(1)) / 1000f
        momentumLastMs = timeMs
        val decay = exp(-dt / MOMENTUM_TAU_S)
        velX *= decay; velY *= decay
        if (hypot(velX, velY) < MOMENTUM_MIN_KEEP) {
            stopMomentum()
            return false
        }
        emitScroll(velX * dt, velY * dt, ScrollPhase.MOMENTUM)
        return true
    }

    /** Ends momentum with a zero-delta MOMENTUM frame so the host can tell the fling is over. */
    private fun stopMomentum() {
        if (!momentum) return
        momentum = false
        sink.scroll(cursorX, cursorY, 0, 0, ScrollPhase.MOMENTUM)
    }

    private fun emitScroll(dx: Float, dy: Float, phase: Int) {
        fracX += dx; fracY += dy
        val ix = fracX.roundToInt()
        val iy = fracY.roundToInt()
        if (ix == 0 && iy == 0 && phase == ScrollPhase.MOMENTUM) return
        fracX -= ix; fracY -= iy
        if (ix != 0 || iy != 0 || phase != ScrollPhase.CHANGE) sink.scroll(cursorX, cursorY, ix, iy, phase)
    }

    private fun click(button: Int) {
        sink.mouse(MouseAction.DOWN, button, cursorX, cursorY)
        sink.mouse(MouseAction.UP, button, cursorX, cursorY)
    }

    fun setCursor(nx: Float, ny: Float) { cursorX = nx.coerceIn(0f, 1f); cursorY = ny.coerceIn(0f, 1f) }

    companion object {
        const val TAP_MS = 250L
        const val DOUBLE_TAP_MS = 300L
        const val BASE_GAIN = 1.25f
        const val ACCEL = 0.35f
        const val MOMENTUM_TAU_S = 0.35f
        const val MOMENTUM_MIN_START = 250f
        const val MOMENTUM_MIN_KEEP = 20f
    }
}
