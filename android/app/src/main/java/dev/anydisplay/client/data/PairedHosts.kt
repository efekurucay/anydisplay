package dev.anydisplay.client.data

import android.content.Context
import dev.anydisplay.client.protocol.PairStore
import dev.anydisplay.client.protocol.hexToBytes
import dev.anydisplay.client.protocol.toHex
import org.json.JSONObject

data class PairedHost(
    val hostIdHex: String,
    val name: String,
    val os: String,
    val pairedAtMs: Long,
    val lastSeenMs: Long,
)

/** Pairing secrets keyed by host_id, kept in app-private storage (PROTOCOL.md 4.2 step 11). */
class PairedHosts(context: Context) : PairStore {
    private val prefs = context.applicationContext.getSharedPreferences("anydisplay_pairs", Context.MODE_PRIVATE)

    @Synchronized
    fun list(): List<PairedHost> = prefs.all.mapNotNull { (k, v) ->
        try {
            val o = JSONObject(v as String)
            PairedHost(k, o.optString("name"), o.optString("os"), o.optLong("paired"), o.optLong("seen"))
        } catch (_: Exception) {
            null
        }
    }.sortedByDescending { it.lastSeenMs }

    @Synchronized
    override fun secretFor(hostId: ByteArray): ByteArray? = prefs.getString(hostId.toHex(), null)?.let {
        try { JSONObject(it).getString("secret").hexToBytes().takeIf { s -> s.size == 32 } } catch (_: Exception) { null }
    }

    @Synchronized
    override fun save(hostId: ByteArray, hostName: String, hostOs: String, secret: ByteArray) {
        val now = System.currentTimeMillis()
        val o = JSONObject().put("name", hostName).put("os", hostOs).put("secret", secret.toHex()).put("paired", now).put("seen", now)
        prefs.edit().putString(hostId.toHex(), o.toString()).commit()
    }

    @Synchronized
    override fun touch(hostId: ByteArray) {
        val key = hostId.toHex()
        val o = prefs.getString(key, null)?.let { JSONObject(it) } ?: return
        prefs.edit().putString(key, o.put("seen", System.currentTimeMillis()).toString()).apply()
    }

    @Synchronized
    fun forget(hostIdHex: String) {
        prefs.edit().remove(hostIdHex).commit()
    }
}
