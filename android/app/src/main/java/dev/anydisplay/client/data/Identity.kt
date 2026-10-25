package dev.anydisplay.client.data

import android.content.Context
import android.os.Build
import android.provider.Settings as SysSettings
import dev.anydisplay.client.protocol.Crypto
import dev.anydisplay.client.protocol.hexToBytes
import dev.anydisplay.client.protocol.toHex

/** Per-installation identity: the random 16-byte device_id (PROTOCOL.md 4.1) and the user-visible device name. */
class Identity(context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("anydisplay_identity", Context.MODE_PRIVATE)

    val deviceId: ByteArray by lazy {
        prefs.getString("device_id", null)?.takeIf { it.length == 32 }?.hexToBytes()
            ?: Crypto.randomBytes(16).also { prefs.edit().putString("device_id", it.toHex()).apply() }
    }

    val deviceIdHex: String get() = deviceId.toHex()

    val deviceName: String
        get() {
            val user = try { SysSettings.Global.getString(app.contentResolver, SysSettings.Global.DEVICE_NAME) } catch (_: Exception) { null }
            // Some vendors keep the model number in device_name and the marketing name in bluetooth_name.
            val bt = try { SysSettings.Secure.getString(app.contentResolver, "bluetooth_name") } catch (_: Exception) { null }
            if (!user.isNullOrBlank() && (user != Build.MODEL || bt.isNullOrBlank())) return user
            if (!bt.isNullOrBlank()) return bt
            val model = Build.MODEL ?: "Android"
            val maker = Build.MANUFACTURER ?: ""
            return if (model.startsWith(maker, ignoreCase = true)) model else "${maker.replaceFirstChar { it.uppercase() }} $model".trim()
        }
}
