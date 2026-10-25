package dev.anydisplay.client.ui

import android.content.Context
import android.graphics.RectF
import android.view.MotionEvent
import android.view.SurfaceView
import android.widget.FrameLayout

/**
 * Full-screen container that letterboxes the video SurfaceView to the stream's aspect ratio and keeps the exact
 * image rect, which input normalization uses. Other children (overlays, menu) fill the container.
 */
class VideoLayout(context: Context) : FrameLayout(context) {
    val surfaceView = SurfaceView(context)
    val videoRect = RectF()
    var streamWidth = 0
        private set
    var streamHeight = 0
        private set
    var onRectChanged: ((RectF) -> Unit)? = null
    var touchHandler: ((MotionEvent) -> Boolean)? = null
    var genericHandler: ((MotionEvent) -> Boolean)? = null

    init {
        setBackgroundColor(0xFF000000.toInt())
        addView(surfaceView, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
        isFocusable = true
        isFocusableInTouchMode = false
    }

    fun setStreamSize(w: Int, h: Int) {
        if (w == streamWidth && h == streamHeight) return
        streamWidth = w
        streamHeight = h
        requestLayout()
    }

    private fun fit(width: Int, height: Int, out: RectF) {
        if (streamWidth <= 0 || streamHeight <= 0) {
            out.set(0f, 0f, width.toFloat(), height.toFloat())
            return
        }
        val scale = minOf(width.toFloat() / streamWidth, height.toFloat() / streamHeight)
        val w = streamWidth * scale
        val h = streamHeight * scale
        val l = ((width - w) / 2f)
        val t = ((height - h) / 2f)
        out.set(l, t, l + w, t + h)
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        super.onMeasure(widthMeasureSpec, heightMeasureSpec)
        val r = RectF()
        fit(measuredWidth, measuredHeight, r)
        surfaceView.measure(
            MeasureSpec.makeMeasureSpec(Math.round(r.width()), MeasureSpec.EXACTLY),
            MeasureSpec.makeMeasureSpec(Math.round(r.height()), MeasureSpec.EXACTLY),
        )
    }

    override fun onLayout(changed: Boolean, left: Int, top: Int, right: Int, bottom: Int) {
        super.onLayout(changed, left, top, right, bottom)
        val r = RectF()
        fit(right - left, bottom - top, r)
        val l = Math.round(r.left)
        val t = Math.round(r.top)
        surfaceView.layout(l, t, l + surfaceView.measuredWidth, t + surfaceView.measuredHeight)
        val exact = RectF(l.toFloat(), t.toFloat(), (l + surfaceView.measuredWidth).toFloat(), (t + surfaceView.measuredHeight).toFloat())
        if (exact != videoRect) {
            videoRect.set(exact)
            onRectChanged?.invoke(videoRect)
        }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean = touchHandler?.invoke(event) ?: false

    override fun onGenericMotionEvent(event: MotionEvent): Boolean =
        genericHandler?.invoke(event) ?: super.onGenericMotionEvent(event)
}
