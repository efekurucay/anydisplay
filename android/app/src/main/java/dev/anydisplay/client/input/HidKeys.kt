package dev.anydisplay.client.input

import android.view.KeyEvent
import dev.anydisplay.client.protocol.KeyMods

/** Android key codes to USB HID keyboard-page usages (HID Usage Tables, section 10). */
object HidKeys {
    const val ENTER = 0x28
    const val ESCAPE = 0x29
    const val BACKSPACE = 0x2A
    const val TAB = 0x2B
    const val DELETE = 0x4C
    const val RIGHT = 0x4F
    const val LEFT = 0x50
    const val DOWN = 0x51
    const val UP = 0x52
    const val A = 0x04

    fun letter(c: Char): Int = A + (c.lowercaseChar() - 'a')

    private val map = HashMap<Int, Int>(160).apply {
        for (i in 0 until 26) put(KeyEvent.KEYCODE_A + i, A + i)
        for (i in 1..9) put(KeyEvent.KEYCODE_1 + i - 1, 0x1E + i - 1)
        put(KeyEvent.KEYCODE_0, 0x27)
        put(KeyEvent.KEYCODE_ENTER, ENTER)
        put(KeyEvent.KEYCODE_ESCAPE, ESCAPE)
        put(KeyEvent.KEYCODE_DEL, BACKSPACE)
        put(KeyEvent.KEYCODE_TAB, TAB)
        put(KeyEvent.KEYCODE_SPACE, 0x2C)
        put(KeyEvent.KEYCODE_MINUS, 0x2D)
        put(KeyEvent.KEYCODE_EQUALS, 0x2E)
        put(KeyEvent.KEYCODE_LEFT_BRACKET, 0x2F)
        put(KeyEvent.KEYCODE_RIGHT_BRACKET, 0x30)
        put(KeyEvent.KEYCODE_BACKSLASH, 0x31)
        put(KeyEvent.KEYCODE_SEMICOLON, 0x33)
        put(KeyEvent.KEYCODE_APOSTROPHE, 0x34)
        put(KeyEvent.KEYCODE_GRAVE, 0x35)
        put(KeyEvent.KEYCODE_COMMA, 0x36)
        put(KeyEvent.KEYCODE_PERIOD, 0x37)
        put(KeyEvent.KEYCODE_SLASH, 0x38)
        put(KeyEvent.KEYCODE_CAPS_LOCK, 0x39)
        for (i in 0 until 12) put(KeyEvent.KEYCODE_F1 + i, 0x3A + i)
        put(KeyEvent.KEYCODE_SYSRQ, 0x46)
        put(KeyEvent.KEYCODE_SCROLL_LOCK, 0x47)
        put(KeyEvent.KEYCODE_BREAK, 0x48)
        put(KeyEvent.KEYCODE_INSERT, 0x49)
        put(KeyEvent.KEYCODE_MOVE_HOME, 0x4A)
        put(KeyEvent.KEYCODE_PAGE_UP, 0x4B)
        put(KeyEvent.KEYCODE_FORWARD_DEL, DELETE)
        put(KeyEvent.KEYCODE_MOVE_END, 0x4D)
        put(KeyEvent.KEYCODE_PAGE_DOWN, 0x4E)
        put(KeyEvent.KEYCODE_DPAD_RIGHT, RIGHT)
        put(KeyEvent.KEYCODE_DPAD_LEFT, LEFT)
        put(KeyEvent.KEYCODE_DPAD_DOWN, DOWN)
        put(KeyEvent.KEYCODE_DPAD_UP, UP)
        put(KeyEvent.KEYCODE_NUM_LOCK, 0x53)
        put(KeyEvent.KEYCODE_NUMPAD_DIVIDE, 0x54)
        put(KeyEvent.KEYCODE_NUMPAD_MULTIPLY, 0x55)
        put(KeyEvent.KEYCODE_NUMPAD_SUBTRACT, 0x56)
        put(KeyEvent.KEYCODE_NUMPAD_ADD, 0x57)
        put(KeyEvent.KEYCODE_NUMPAD_ENTER, 0x58)
        for (i in 1..9) put(KeyEvent.KEYCODE_NUMPAD_1 + i - 1, 0x59 + i - 1)
        put(KeyEvent.KEYCODE_NUMPAD_0, 0x62)
        put(KeyEvent.KEYCODE_NUMPAD_DOT, 0x63)
        put(KeyEvent.KEYCODE_NUMPAD_EQUALS, 0x67)
        put(KeyEvent.KEYCODE_NUMPAD_COMMA, 0x85)
        put(KeyEvent.KEYCODE_MENU, 0x65)
        put(KeyEvent.KEYCODE_MUTE, 0x7F)
        put(KeyEvent.KEYCODE_CTRL_LEFT, 0xE0)
        put(KeyEvent.KEYCODE_SHIFT_LEFT, 0xE1)
        put(KeyEvent.KEYCODE_ALT_LEFT, 0xE2)
        put(KeyEvent.KEYCODE_META_LEFT, 0xE3)
        put(KeyEvent.KEYCODE_CTRL_RIGHT, 0xE4)
        put(KeyEvent.KEYCODE_SHIFT_RIGHT, 0xE5)
        put(KeyEvent.KEYCODE_ALT_RIGHT, 0xE6)
        put(KeyEvent.KEYCODE_META_RIGHT, 0xE7)
    }

    /** HID usage for [keyCode], or 0 when the key has no HID equivalent. */
    fun usage(keyCode: Int): Int = map[keyCode] ?: 0

    /** PROTOCOL.md KEY modifier bits from an Android meta state. */
    fun modifiers(metaState: Int): Int {
        var m = 0
        if (metaState and KeyEvent.META_CTRL_ON != 0) m = m or KeyMods.CTRL
        if (metaState and KeyEvent.META_SHIFT_ON != 0) m = m or KeyMods.SHIFT
        if (metaState and KeyEvent.META_ALT_ON != 0) m = m or KeyMods.ALT
        if (metaState and KeyEvent.META_META_ON != 0) m = m or KeyMods.META
        return m
    }
}
