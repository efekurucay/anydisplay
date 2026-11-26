package com.usethatscreen.client

import android.util.Log

/** Single log tag for the whole app: `adb logcat -s UseThatScreen`. */
object Elog {
    const val TAG = "UseThatScreen"
    fun d(msg: String) = Log.d(TAG, msg)
    fun i(msg: String) = Log.i(TAG, msg)
    fun w(msg: String, t: Throwable? = null) = Log.w(TAG, msg, t)
    fun e(msg: String, t: Throwable? = null) = Log.e(TAG, msg, t)
}
