package dev.anydisplay.client.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.view.View
import dev.anydisplay.client.session.CursorSink

/** Draws the host cursor (CURSOR_SHAPE at CURSOR_POS, stream pixels) above the video when the host grants it. */
class CursorOverlay(context: Context) : View(context), CursorSink {
    private val lock = Any()
    private var bitmap: Bitmap? = null
    private var hotX = 0
    private var hotY = 0
    @Volatile private var posX = 0
    @Volatile private var posY = 0
    @Volatile private var visible = false
    private val paint = Paint(Paint.FILTER_BITMAP_FLAG)
    private val dst = RectF()
    var videoRect = RectF()
    var streamWidth = 1
    var streamHeight = 1
    var onPosition: ((Float, Float) -> Unit)? = null

    override fun onCursorPos(x: Int, y: Int, visible: Boolean) {
        posX = x
        posY = y
        this.visible = visible
        onPosition?.invoke(x.toFloat() / streamWidth.coerceAtLeast(1), y.toFloat() / streamHeight.coerceAtLeast(1))
        postInvalidateOnAnimation()
    }

    override fun onCursorShape(w: Int, h: Int, hotX: Int, hotY: Int, rgba: ByteArray, offset: Int) {
        // RGBA8 straight alpha -> ARGB ints; setPixels premultiplies.
        val px = IntArray(w * h)
        var o = offset
        for (i in px.indices) {
            val r = rgba[o].toInt() and 0xFF
            val g = rgba[o + 1].toInt() and 0xFF
            val b = rgba[o + 2].toInt() and 0xFF
            val a = rgba[o + 3].toInt() and 0xFF
            px[i] = (a shl 24) or (r shl 16) or (g shl 8) or b
            o += 4
        }
        val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
        bmp.setPixels(px, 0, w, 0, 0, w, h)
        synchronized(lock) {
            bitmap = bmp
            this.hotX = hotX
            this.hotY = hotY
        }
        postInvalidateOnAnimation()
    }

    fun clear() {
        synchronized(lock) { bitmap = null }
        visible = false
        postInvalidateOnAnimation()
    }

    override fun onDraw(canvas: Canvas) {
        if (!visible) return
        val bmp: Bitmap
        val hx: Int
        val hy: Int
        synchronized(lock) {
            bmp = bitmap ?: return
            hx = hotX
            hy = hotY
        }
        val scale = videoRect.width() / streamWidth.coerceAtLeast(1)
        val x = videoRect.left + (posX - hx) * scale
        val y = videoRect.top + (posY - hy) * scale
        dst.set(x, y, x + bmp.width * scale, y + bmp.height * scale)
        canvas.drawBitmap(bmp, null, dst, paint)
    }
}
