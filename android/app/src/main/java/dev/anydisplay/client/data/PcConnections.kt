package dev.anydisplay.client.data

import android.content.Context
import org.json.JSONObject
import java.net.InetAddress

fun parsePcAddress(input: String): InetAddress? {
    val fields = input.trim().split('.')
    if (fields.size != 4) return null
    val octets = fields.map { field ->
        if (field.any { it !in '0'..'9' }) return null
        field.toIntOrNull()?.takeIf { it in 0..255 } ?: return null
    }
    val address = InetAddress.getByAddress(ByteArray(4) { octets[it].toByte() })
    val sharedAddress = octets[0] == 100 && octets[1] in 64..127
    return address.takeIf { it.isSiteLocalAddress || sharedAddress }
}

data class PcConnection(val address: String, val name: String, val lastUsed: Long)

class PcConnections(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences("anydisplay_pc_connections", Context.MODE_PRIVATE)

    fun list(): List<PcConnection> = prefs.all.mapNotNull { (address, value) ->
        read(address, value as? String)
    }.sortedByDescending { it.lastUsed }

    fun remember(address: String, hostName: String) = synchronized(prefs) {
        val name = read(address, prefs.getString(address, null))?.name ?: hostName.ifBlank { address }
        save(PcConnection(address, name, System.currentTimeMillis()))
    }

    fun rename(address: String, name: String) = synchronized(prefs) {
        read(address, prefs.getString(address, null))?.let { save(it.copy(name = name)) }
    }

    fun remove(address: String) = synchronized(prefs) {
        prefs.edit().remove(address).apply()
    }

    private fun read(address: String, value: String?): PcConnection? = value?.let {
        runCatching {
            val entry = JSONObject(it)
            PcConnection(address, entry.getString("name"), entry.getLong("lastUsed"))
        }.getOrNull()
    }

    private fun save(entry: PcConnection) {
        val value = JSONObject().put("name", entry.name).put("lastUsed", entry.lastUsed)
        prefs.edit().putString(entry.address, value.toString()).apply()
    }
}
