package dev.anydisplay.client.input

import android.graphics.RectF
import android.os.SystemClock
import android.view.MotionEvent
import android.view.View
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import dev.anydisplay.client.data.TouchMode
import dev.anydisplay.client.protocol.ByteReader
import dev.anydisplay.client.protocol.PenAction
import dev.anydisplay.client.protocol.Proto
import dev.anydisplay.client.protocol.normalize
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import kotlin.math.PI
import kotlin.math.abs

/** Feeds real stylus/finger MotionEvents through [InputRouter] on a device and checks the PEN/TOUCH bytes. */
@RunWith(AndroidJUnit4::class)
class InputRouterDeviceTest {
    private class Sent(val type: Int, val body: ByteArray)
    private class Pen(val action: Int, val tool: Int, val buttons: Int, val x: Int, val y: Int, val p: Int, val tx: Int, val ty: Int, val t: Long)

    private val sent = ArrayList<Sent>()
    private val router: InputRouter by lazy {
        val ctx = InstrumentationRegistry.getInstrumentation().targetContext
        InputRouter(View(ctx), RectF(100f, 0f, 1100f, 500f), { t, b -> sent += Sent(t, b) }, 10f).apply {
            mode = TouchMode.DIRECT
            palmRejection = true
            streamWidth = 2000; streamHeight = 1000
            onGeometryChanged()
        }
    }

    private fun event(
        action: Int, tool: Int, x: Float, y: Float, pressure: Float = 0.5f, tilt: Float = 0f, orientation: Float = 0f,
        buttons: Int = 0, time: Long = SystemClock.uptimeMillis(),
    ): MotionEvent {
        val props = arrayOf(MotionEvent.PointerProperties().apply { id = 0; toolType = tool })
        val coords = arrayOf(MotionEvent.PointerCoords().apply {
            this.x = x; this.y = y; this.pressure = pressure; size = 0.1f
            setAxisValue(MotionEvent.AXIS_TILT, tilt)
            setAxisValue(MotionEvent.AXIS_ORIENTATION, orientation)
        })
        return MotionEvent.obtain(time, time, action, 1, props, coords, 0, buttons, 1f, 1f, 0, 0, 0x4002 /* stylus */, 0)
    }

    private fun pens(): List<Pen> = sent.filter { it.type == Proto.PEN }.flatMap { s ->
        val r = ByteReader(s.body)
        List(r.u8()) {
            val pen = Pen(r.u8(), r.u8(), r.u8(), r.u16(), r.u16(), r.u16(), r.i16(), r.i16(), 0)
            r.u16() // twist
            Pen(pen.action, pen.tool, pen.buttons, pen.x, pen.y, pen.p, pen.tx, pen.ty, r.u32())
        }
    }

    @Test
    fun stylusStrokeWithHistoryTiltAndButtons() {
        val t0 = SystemClock.uptimeMillis()
        router.onTouchEvent(event(MotionEvent.ACTION_DOWN, MotionEvent.TOOL_TYPE_STYLUS, 600f, 250f, 0.5f,
            (PI / 6).toFloat(), (-PI / 2).toFloat(), MotionEvent.BUTTON_STYLUS_PRIMARY, t0))
        val move = event(MotionEvent.ACTION_MOVE, MotionEvent.TOOL_TYPE_STYLUS, 200f, 100f, 0.2f, time = t0 + 10)
        val c = MotionEvent.PointerCoords().apply { x = 300f; y = 120f; pressure = 0.3f }
        move.addBatch(t0 + 20, arrayOf(c), 0)
        val c2 = MotionEvent.PointerCoords().apply { x = 400f; y = 140f; pressure = 0.4f }
        move.addBatch(t0 + 30, arrayOf(c2), 0)
        router.onTouchEvent(move)
        router.onTouchEvent(event(MotionEvent.ACTION_UP, MotionEvent.TOOL_TYPE_ERASER, 400f, 140f, 0f, time = t0 + 40))

        val p = pens()
        assertEquals(listOf(PenAction.DOWN, PenAction.MOVE, PenAction.MOVE, PenAction.MOVE, PenAction.UP), p.map { it.action })
        // Down: centre of the rect, half pressure, 30 degrees toward +X (pointing left), barrel button.
        assertEquals(32768, p[0].x); assertEquals(32768, p[0].y)
        assertTrue(abs(p[0].p - 32767) < 2)
        assertEquals(3000, p[0].tx); assertEquals(0, p[0].ty)
        assertEquals(1, p[0].buttons)
        // History oldest first, then the current sample; timestamps follow.
        assertEquals(listOf(200f, 300f, 400f).map { normalize(it, 100f, 1000f) }, p.subList(1, 4).map { it.x })
        assertEquals(listOf(t0 + 10, t0 + 20, t0 + 30).map { it and 0xFFFFFFFFL }, p.subList(1, 4).map { it.t })
        assertEquals(1, p[4].tool) // eraser
        assertEquals(0, p[4].p)
    }

    @Test
    fun hoverAndPalmRejection() {
        router.onGenericMotionEvent(event(MotionEvent.ACTION_HOVER_ENTER, MotionEvent.TOOL_TYPE_STYLUS, 500f, 200f, 0f))
        router.onGenericMotionEvent(event(MotionEvent.ACTION_HOVER_MOVE, MotionEvent.TOOL_TYPE_STYLUS, 510f, 200f, 0f))
        // A palm lands while the pen hovers: no TOUCH may reach the host.
        router.onTouchEvent(event(MotionEvent.ACTION_DOWN, MotionEvent.TOOL_TYPE_FINGER, 700f, 300f))
        router.onTouchEvent(event(MotionEvent.ACTION_UP, MotionEvent.TOOL_TYPE_FINGER, 700f, 300f))
        router.onGenericMotionEvent(event(MotionEvent.ACTION_HOVER_EXIT, MotionEvent.TOOL_TYPE_STYLUS, 510f, 200f, 0f))
        assertEquals(listOf(PenAction.HOVER_ENTER, PenAction.HOVER_MOVE, PenAction.HOVER_EXIT), pens().map { it.action })
        assertTrue(pens().all { it.p == 0 })
        assertTrue(sent.none { it.type == Proto.TOUCH })
        // 300 ms after the pen leaves, fingers work again.
        Thread.sleep(InputRouter.PALM_HOLD_MS + 50)
        router.onTouchEvent(event(MotionEvent.ACTION_DOWN, MotionEvent.TOOL_TYPE_FINGER, 700f, 300f))
        assertEquals(1, sent.count { it.type == Proto.TOUCH })
    }
}
