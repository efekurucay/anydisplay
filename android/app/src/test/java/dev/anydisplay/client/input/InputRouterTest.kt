package dev.anydisplay.client.input

import android.graphics.RectF
import android.view.MotionEvent
import android.view.View
import dev.anydisplay.client.data.TouchMode
import dev.anydisplay.client.protocol.MouseAction
import dev.anydisplay.client.protocol.PenAction
import dev.anydisplay.client.protocol.Proto
import dev.anydisplay.client.protocol.TouchAction
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], manifest = Config.NONE)
class InputRouterTest {
    private data class Sent(val type: Int, val body: ByteArray)
    private val sent = ArrayList<Sent>()
    private val router = InputRouter(
        View(RuntimeEnvironment.getApplication()), RectF(0f, 0f, 1000f, 500f),
        { type, body -> sent += Sent(type, body) }, 10f,
    ).apply {
        palmRejection = false
        onGeometryChanged()
    }

    private fun event(
        action: Int, time: Long, fingerX: Float = 300f, mixed: Boolean = true,
        penTool: Int = MotionEvent.TOOL_TYPE_STYLUS, penFirst: Boolean = true,
    ) {
        val tools = if (!mixed) listOf(penTool)
            else if (penFirst) listOf(penTool, MotionEvent.TOOL_TYPE_FINGER)
            else listOf(MotionEvent.TOOL_TYPE_FINGER, penTool)
        val props = tools.map { tool ->
            MotionEvent.PointerProperties().apply {
                id = if (tool == MotionEvent.TOOL_TYPE_FINGER) 1 else 0
                toolType = tool
            }
        }.toTypedArray()
        val coords = tools.map { tool ->
            MotionEvent.PointerCoords().apply {
                x = if (tool == MotionEvent.TOOL_TYPE_FINGER) fingerX else 100f
                y = 100f; pressure = 0.5f; size = 0.1f
            }
        }.toTypedArray()
        val ev = MotionEvent.obtain(1000, time, action, tools.size, props, coords,
            0, 0, 1f, 1f, 0, 0, 0x4002 /* stylus */, 0)
        try {
            router.onTouchEvent(ev)
        } finally {
            ev.recycle()
        }
    }

    private fun penActions() = sent.filter { it.type == Proto.PEN }.map { it.body[1].toInt() }
    private fun touchActions() = sent.filter { it.type == Proto.TOUCH }.map { it.body[2].toInt() }

    private fun startMixedGesture() {
        event(MotionEvent.ACTION_DOWN, 1000, mixed = false)
        event(MotionEvent.ACTION_POINTER_DOWN or (1 shl 8), 1010)
    }

    @Test
    fun fingerTransitionsDoNotPressOrReleasePen() {
        startMixedGesture()
        event(MotionEvent.ACTION_POINTER_UP or (1 shl 8), 1020)
        assertEquals(listOf(PenAction.DOWN), penActions())
        assertEquals(listOf(TouchAction.DOWN, TouchAction.UP), touchActions())
    }

    @Test
    fun mixedMoveUpdatesFingerAndPen() {
        startMixedGesture()
        event(MotionEvent.ACTION_MOVE, 1060, fingerX = 700f)
        assertEquals(listOf(PenAction.DOWN, PenAction.MOVE), penActions())
        assertEquals(listOf(TouchAction.DOWN, TouchAction.MOVE), touchActions())
        val body = sent.last { it.type == Proto.TOUCH }.body
        assertEquals(1, body[0].toInt()) // only the finger, never the stylus
        assertEquals(1, body[1].toInt()) // stable finger id
        assertEquals(45875, (body[3].toInt() and 255) or ((body[4].toInt() and 255) shl 8))
    }

    @Test
    fun mixedCancelReleasesFingerAndPen() {
        startMixedGesture()
        event(MotionEvent.ACTION_CANCEL, 1050)
        assertEquals(listOf(PenAction.DOWN, PenAction.CANCEL), penActions())
        assertEquals(listOf(TouchAction.DOWN, TouchAction.CANCEL), touchActions())
        sent.clear()
        router.cancelFingers()
        assertTrue(sent.isEmpty())
    }

    @Test
    fun trackpadDragDuringPenContactMovesWithoutClicking() {
        router.mode = TouchMode.TRACKPAD
        startMixedGesture()
        event(MotionEvent.ACTION_MOVE, 1060, fingerX = 700f)
        event(MotionEvent.ACTION_POINTER_UP or (1 shl 8), 1100, fingerX = 700f)
        assertTrue(router.trackpadCursor.first > 0.5f)
        assertEquals(listOf(MouseAction.MOVE), sent.filter { it.type == Proto.MOUSE }.map { it.body[0].toInt() })
        assertEquals(listOf(PenAction.DOWN, PenAction.MOVE), penActions())
    }

    @Test
    fun palmRejectionSuppressesMixedFingerEvents() {
        router.palmRejection = true
        startMixedGesture()
        event(MotionEvent.ACTION_MOVE, 1060)
        event(MotionEvent.ACTION_POINTER_UP or (1 shl 8), 1100)
        event(MotionEvent.ACTION_CANCEL, 1110, mixed = false)
        assertEquals(listOf(PenAction.DOWN, PenAction.MOVE, PenAction.CANCEL), penActions())
        assertTrue(sent.none { it.type == Proto.TOUCH || it.type == Proto.MOUSE })
    }

    @Test
    fun eraserAtSecondIndexOwnsMoveAndUp() {
        event(MotionEvent.ACTION_POINTER_DOWN or (1 shl 8), 1010,
            penTool = MotionEvent.TOOL_TYPE_ERASER, penFirst = false)
        event(MotionEvent.ACTION_MOVE, 1060, penTool = MotionEvent.TOOL_TYPE_ERASER, penFirst = false)
        event(MotionEvent.ACTION_POINTER_UP or (1 shl 8), 1100,
            penTool = MotionEvent.TOOL_TYPE_ERASER, penFirst = false)
        assertEquals(listOf(PenAction.DOWN, PenAction.MOVE, PenAction.UP), penActions())
        assertTrue(sent.filter { it.type == Proto.PEN }.all { it.body[2].toInt() == 1 })
    }

    @Test
    fun stylusOnlyMoveDoesNotSendTouchFrames() {
        event(MotionEvent.ACTION_DOWN, 1000, mixed = false)
        event(MotionEvent.ACTION_MOVE, 1060, mixed = false)
        event(MotionEvent.ACTION_UP, 1100, mixed = false)
        assertEquals(listOf(PenAction.DOWN, PenAction.MOVE, PenAction.UP), penActions())
        assertTrue(sent.none { it.type == Proto.TOUCH })
    }
}
