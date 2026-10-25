package dev.anydisplay.client.ui

import android.annotation.SuppressLint
import android.content.Context
import android.content.res.ColorStateList
import android.util.TypedValue
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.LinearLayout
import android.widget.TextView
import com.google.android.material.card.MaterialCardView
import dev.anydisplay.client.R
import kotlin.math.abs

/** Draggable, collapsible tool strip shown above the stream. The handle drags it and toggles it open. */
@SuppressLint("ViewConstructor")
class FloatingMenu(context: Context, private val onMoved: (Float, Float) -> Unit) : MaterialCardView(context) {
    class Item(val id: String, val icon: Int, val label: String, val text: String? = null, val onClick: () -> Unit)

    private val column = LinearLayout(context).apply { orientation = LinearLayout.VERTICAL; gravity = Gravity.CENTER_HORIZONTAL }
    private val itemsBox = LinearLayout(context).apply { orientation = LinearLayout.VERTICAL; gravity = Gravity.CENTER_HORIZONTAL }
    private val handle: ImageButton
    private val buttons = HashMap<String, View>()
    private val dp = resources.displayMetrics.density
    private val fg = 0xFFE8EAF6.toInt()
    var expanded = false
        private set

    init {
        radius = 24 * dp
        cardElevation = 6 * dp
        setCardBackgroundColor(0xE61C1F2A.toInt())
        strokeWidth = 0
        handle = iconButton(R.drawable.ic_menu, context.getString(R.string.menu_toggle))
        column.addView(handle)
        column.addView(itemsBox)
        itemsBox.visibility = View.GONE
        addView(column, FrameLayout.LayoutParams(FrameLayout.LayoutParams.WRAP_CONTENT, FrameLayout.LayoutParams.WRAP_CONTENT))
        setupDrag()
    }

    private fun iconButton(icon: Int, label: String): ImageButton = ImageButton(context).apply {
        setImageResource(icon)
        imageTintList = ColorStateList.valueOf(fg)
        contentDescription = label
        tooltipText = label
        val out = TypedValue()
        context.theme.resolveAttribute(android.R.attr.selectableItemBackgroundBorderless, out, true)
        setBackgroundResource(out.resourceId)
        val s = (48 * dp).toInt()
        layoutParams = LinearLayout.LayoutParams(s, s)
    }

    fun setItems(items: List<Item>) {
        itemsBox.removeAllViews()
        buttons.clear()
        for (item in items) {
            val v: View = if (item.text != null) {
                TextView(context).apply {
                    text = item.text
                    setTextColor(fg)
                    textSize = 13f
                    gravity = Gravity.CENTER
                    contentDescription = item.label
                    val out = TypedValue()
                    context.theme.resolveAttribute(android.R.attr.selectableItemBackgroundBorderless, out, true)
                    setBackgroundResource(out.resourceId)
                    val s = (48 * dp).toInt()
                    layoutParams = LinearLayout.LayoutParams(s, s)
                }
            } else iconButton(item.icon, item.label)
            v.setOnClickListener { item.onClick() }
            itemsBox.addView(v)
            buttons[item.id] = v
        }
    }

    fun setIcon(id: String, icon: Int, label: String) {
        (buttons[id] as? ImageButton)?.apply {
            setImageResource(icon)
            contentDescription = label
            tooltipText = label
        }
    }

    /** Lays the strip out along the longer side of the screen so every button fits. */
    private fun orient() {
        val p = parent as? View ?: return
        val o = if (p.height >= p.width) LinearLayout.VERTICAL else LinearLayout.HORIZONTAL
        column.orientation = o
        itemsBox.orientation = o
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        (parent as? View)?.addOnLayoutChangeListener { _, l, t, r, b, ol, ot, or, ob ->
            if (r - l != or - ol || b - t != ob - ot) post { orient(); clampIntoParent() }
        }
    }

    fun toggleExpanded() {
        orient()
        expanded = !expanded
        itemsBox.visibility = if (expanded) View.VISIBLE else View.GONE
        handle.setImageResource(if (expanded) R.drawable.ic_close else R.drawable.ic_menu)
        post { clampIntoParent() }
    }

    fun clampIntoParent() {
        val p = parent as? View ?: return
        translationX = translationX.coerceIn(0f, (p.width - width).toFloat().coerceAtLeast(0f))
        translationY = translationY.coerceIn(0f, (p.height - height).toFloat().coerceAtLeast(0f))
    }

    @SuppressLint("ClickableViewAccessibility")
    private fun setupDrag() {
        val slop = ViewConfiguration.get(context).scaledTouchSlop
        var downX = 0f
        var downY = 0f
        var startTx = 0f
        var startTy = 0f
        var dragging = false
        handle.setOnTouchListener { _, e ->
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    downX = e.rawX; downY = e.rawY; startTx = translationX; startTy = translationY; dragging = false
                    handle.isPressed = true
                }
                MotionEvent.ACTION_MOVE -> {
                    val dx = e.rawX - downX
                    val dy = e.rawY - downY
                    if (!dragging && (abs(dx) > slop || abs(dy) > slop)) dragging = true
                    if (dragging) {
                        translationX = startTx + dx
                        translationY = startTy + dy
                        clampIntoParent()
                    }
                }
                MotionEvent.ACTION_UP -> {
                    handle.isPressed = false
                    if (dragging) onMoved(translationX, translationY) else toggleExpanded()
                }
                MotionEvent.ACTION_CANCEL -> handle.isPressed = false
            }
            true
        }
    }
}
