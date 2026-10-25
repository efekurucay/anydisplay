package dev.anydisplay.client.input

import android.view.KeyEvent
import dev.anydisplay.client.protocol.KeyMods
import dev.anydisplay.client.protocol.MouseAction
import dev.anydisplay.client.protocol.MouseButton
import dev.anydisplay.client.protocol.ScrollPhase
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.abs

class InputLogicTest {
    @Test
    fun tiltRoundTripsAndroidMapping() {
        for (txDeg in -60..60 step 15) for (tyDeg in -60..60 step 15) {
            val (tilt, orientation) = PenMath.androidAxes(txDeg * PI / 180, tyDeg * PI / 180)
            val (cx, cy) = PenMath.tiltCentidegrees(tilt.toFloat(), orientation.toFloat())
            assertTrue("tx $txDeg -> $cx", abs(cx - txDeg * 100) <= 2)
            assertTrue("ty $tyDeg -> $cy", abs(cy - tyDeg * 100) <= 2)
        }
    }

    @Test
    fun tiltDirections() {
        // Android: orientation 0 = pointing up -> pen leans toward the user (+Y).
        assertEquals(0 to 3000, PenMath.tiltCentidegrees((PI / 6).toFloat(), 0f))
        // Pointing left (-π/2) -> pen leans to the right (+X).
        assertEquals(3000 to 0, PenMath.tiltCentidegrees((PI / 6).toFloat(), (-PI / 2).toFloat()))
        assertEquals(0 to 0, PenMath.tiltCentidegrees(0f, 1f))
    }

    @Test
    fun hidMapping() {
        assertEquals(0x04, HidKeys.usage(KeyEvent.KEYCODE_A))
        assertEquals(0x1D, HidKeys.usage(KeyEvent.KEYCODE_Z))
        assertEquals(0x1E, HidKeys.usage(KeyEvent.KEYCODE_1))
        assertEquals(0x27, HidKeys.usage(KeyEvent.KEYCODE_0))
        assertEquals(0x2A, HidKeys.usage(KeyEvent.KEYCODE_DEL))
        assertEquals(0x45, HidKeys.usage(KeyEvent.KEYCODE_F12))
        assertEquals(0x52, HidKeys.usage(KeyEvent.KEYCODE_DPAD_UP))
        assertEquals(0xE3, HidKeys.usage(KeyEvent.KEYCODE_META_LEFT))
        assertEquals(0, HidKeys.usage(KeyEvent.KEYCODE_VOLUME_UP))
        assertEquals(KeyMods.CTRL or KeyMods.SHIFT, HidKeys.modifiers(KeyEvent.META_CTRL_ON or KeyEvent.META_SHIFT_ON))
    }

    private class Rec : TrackpadEngine.Sink {
        val events = ArrayList<String>()
        override fun mouse(action: Int, button: Int, nx: Float, ny: Float) { events += "M$action/$button" }
        override fun scroll(nx: Float, ny: Float, dx: Int, dy: Int, phase: Int) { events += "S$phase($dx,$dy)" }
    }

    private fun engine(rec: Rec) = TrackpadEngine(rec, 10f).apply { setGeometry(1000f, 500f, 2000, 1000) }

    @Test
    fun tapClicksLeft() {
        val r = Rec()
        val e = engine(r)
        e.down(0, 100f, 100f, 0)
        e.up(0, 100)
        assertEquals(listOf("M${MouseAction.DOWN}/${MouseButton.LEFT}", "M${MouseAction.UP}/${MouseButton.LEFT}"), r.events)
    }

    @Test
    fun moveMovesCursorRelatively() {
        val r = Rec()
        val e = engine(r)
        e.down(0, 100f, 100f, 0)
        var t = 0L
        for (i in 1..10) {
            val prev = mapOf(0 to (100f + (i - 1) * 10 to 100f))
            e.move(0, 100f + i * 10, 100f)
            t += 16
            e.commitMove(t, prev)
        }
        e.up(0, t + 300)
        assertTrue(r.events.all { it == "M${MouseAction.MOVE}/0" })
        assertTrue(e.cursorX > 0.55f)
        assertEquals(0.5f, e.cursorY, 1e-6f)
    }

    @Test
    fun twoFingerTapClicksRight() {
        val r = Rec()
        val e = engine(r)
        e.down(0, 100f, 100f, 0)
        e.down(1, 200f, 100f, 20)
        e.up(0, 120)
        e.up(1, 130)
        assertEquals(listOf("M1/${MouseButton.RIGHT}", "M2/${MouseButton.RIGHT}"), r.events)
    }

    @Test
    fun twoFingerDragScrollsWithPhasesAndMomentum() {
        val r = Rec()
        val e = engine(r)
        e.down(0, 100f, 100f, 0)
        e.down(1, 200f, 100f, 0)
        var t = 0L
        for (i in 1..10) {
            val prev = mapOf(0 to (100f to 100f + (i - 1) * 20), 1 to (200f to 100f + (i - 1) * 20))
            e.move(0, 100f, 100f + i * 20)
            e.move(1, 200f, 100f + i * 20)
            t += 16
            e.commitMove(t, prev)
        }
        e.up(0, t + 10)
        e.up(1, t + 12)
        assertEquals("S${ScrollPhase.BEGIN}(0,0)", r.events.first())
        val changes = r.events.filter { it.startsWith("S${ScrollPhase.CHANGE}") }
        assertTrue(changes.isNotEmpty())
        // Fingers moving down: content moves down, dy positive, in stream pixels (2x view).
        assertTrue(changes.all { it.contains(",4") })
        assertTrue(r.events.contains("S${ScrollPhase.END}(0,0)"))
        assertTrue(e.momentumActive)
        var n = 0
        while (e.tick(t + 16L * ++n) && n < 1000) Unit
        assertTrue(r.events.last() == "S${ScrollPhase.MOMENTUM}(0,0)")
        assertTrue(r.events.count { it.startsWith("S${ScrollPhase.MOMENTUM}") } > 3)
    }

    @Test
    fun tapThenDragHoldsLeftButton() {
        val r = Rec()
        val e = engine(r)
        e.down(0, 100f, 100f, 0)
        e.up(0, 80)
        e.down(0, 100f, 100f, 200)
        e.move(0, 150f, 100f)
        e.commitMove(220, mapOf(0 to (100f to 100f)))
        e.up(0, 400)
        assertEquals(listOf("M1/0", "M2/0", "M1/0", "M0/0", "M2/0"), r.events)
    }
}
