package dev.anydisplay.client.ui

import android.annotation.SuppressLint
import android.content.Context
import android.text.InputType
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import dev.anydisplay.client.input.HidKeys

/**
 * Invisible text editor target for the soft keyboard. Committed and composing text is diffed against what was already
 * sent, so the host sees live typing as TEXT plus Backspace KEYs; editor actions become Enter.
 */
@SuppressLint("ViewConstructor")
class KeyboardSink(context: Context, private val out: Output) : View(context) {
    interface Output {
        fun text(s: String)
        fun tap(usage: Int)
    }

    init {
        isFocusable = true
        isFocusableInTouchMode = true
    }

    override fun onCheckIsTextEditor() = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or
            InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_ACTION_NONE
        return Connection()
    }

    private inner class Connection : BaseInputConnection(this, false) {
        private var composing = ""

        private fun replaceComposing(next: String) {
            var p = 0
            val max = minOf(composing.length, next.length)
            while (p < max && composing[p] == next[p]) p++
            if (p > 0 && p < composing.length && Character.isHighSurrogate(composing[p - 1])) p--
            val removed = composing.codePointCount(p, composing.length)
            repeat(removed) { out.tap(HidKeys.BACKSPACE) }
            val added = next.substring(p)
            if (added.isNotEmpty()) emit(added)
            composing = next
        }

        private fun emit(s: String) {
            var start = 0
            for (i in s.indices) {
                if (s[i] == '\n') {
                    if (i > start) out.text(s.substring(start, i))
                    out.tap(HidKeys.ENTER)
                    start = i + 1
                }
            }
            if (start < s.length) out.text(s.substring(start))
        }

        override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
            replaceComposing(text?.toString() ?: "")
            composing = ""
            return true
        }

        override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
            replaceComposing(text?.toString() ?: "")
            return true
        }

        override fun finishComposingText(): Boolean {
            composing = ""
            return true
        }

        override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
            repeat(beforeLength.coerceAtMost(64)) { out.tap(HidKeys.BACKSPACE) }
            repeat(afterLength.coerceAtMost(64)) { out.tap(HidKeys.DELETE) }
            return true
        }

        override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int) =
            deleteSurroundingText(beforeLength, afterLength)

        override fun performEditorAction(actionCode: Int): Boolean {
            out.tap(HidKeys.ENTER)
            return true
        }

        override fun getTextBeforeCursor(n: Int, flags: Int): CharSequence = composing.takeLast(n)
        override fun getTextAfterCursor(n: Int, flags: Int): CharSequence = ""
    }
}
