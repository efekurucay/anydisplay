package com.usethatscreen.client.ui

import android.content.Context
import android.graphics.RectF
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.ScaleGestureDetector
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
    var zoomControls = false
    private var navigating = false
    private var zoom = 1f
    private val fitted = RectF()
    private var offsetX = 0f
    private var offsetY = 0f
    private val pinch = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
        override fun onScale(detector: ScaleGestureDetector): Boolean {
            val next = (zoom * detector.scaleFactor).coerceIn(1f, 4f)
            val ratio = next / zoom
            offsetX = detector.focusX - fitted.left - (detector.focusX - videoRect.left) * ratio
            offsetY = detector.focusY - fitted.top - (detector.focusY - videoRect.top) * ratio
            zoom = next
            updateViewport()
            return true
        }
    }).apply {
        isQuickScaleEnabled = false
        isStylusScaleEnabled = false
    }
    private val pan = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent) = true

        override fun onScroll(e1: MotionEvent?, e2: MotionEvent, distanceX: Float, distanceY: Float): Boolean {
            if (e2.pointerCount != 1) return false
            offsetX -= distanceX
            offsetY -= distanceY
            updateViewport()
            return true
        }
    })

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
        if (fitted.left != l.toFloat() || fitted.top != t.toFloat() ||
            fitted.right != surfaceView.right.toFloat() || fitted.bottom != surfaceView.bottom.toFloat()) {
            fitted.set(l.toFloat(), t.toFloat(), surfaceView.right.toFloat(), surfaceView.bottom.toFloat())
            resetZoom()
        }
    }

    fun resetZoom() {
        zoom = 1f
        offsetX = 0f
        offsetY = 0f
        updateViewport()
    }

    private fun updateViewport() {
        fun position(offset: Float, start: Float, size: Float, viewport: Int): Float {
            if (zoom == 1f) return 0f
            return if (size <= viewport) (viewport - size) / 2f - start
            else offset.coerceIn(viewport - size - start, -start)
        }
        val w = fitted.width() * zoom
        val h = fitted.height() * zoom
        offsetX = position(offsetX, fitted.left, w, width)
        offsetY = position(offsetY, fitted.top, h, height)
        surfaceView.pivotX = 0f
        surfaceView.pivotY = 0f
        surfaceView.scaleX = zoom
        surfaceView.scaleY = zoom
        surfaceView.translationX = offsetX
        surfaceView.translationY = offsetY
        val left = fitted.left + offsetX
        val top = fitted.top + offsetY
        videoRect.set(left, top, left + w, top + h)
        onRectChanged?.invoke(videoRect)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (event.actionMasked == MotionEvent.ACTION_DOWN) {
            navigating = zoomControls && event.getToolType(0) == MotionEvent.TOOL_TYPE_FINGER
        }
        if (!navigating) return touchHandler?.invoke(event) ?: false
        pinch.onTouchEvent(event)
        pan.onTouchEvent(event)
        return true
    }

    override fun onGenericMotionEvent(event: MotionEvent): Boolean =
        genericHandler?.invoke(event) ?: super.onGenericMotionEvent(event)
}
