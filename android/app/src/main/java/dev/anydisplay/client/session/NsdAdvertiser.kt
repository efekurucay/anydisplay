package dev.anydisplay.client.session

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import dev.anydisplay.client.Elog
import dev.anydisplay.client.protocol.Proto

/** Advertises `_anydisplay._tcp` with TXT `id=<device_id hex>` and `v=1` (PROTOCOL.md section 2). */
class NsdAdvertiser(context: Context) {
    private val nsd = context.applicationContext.getSystemService(Context.NSD_SERVICE) as NsdManager
    private var listener: NsdManager.RegistrationListener? = null

    fun start(instanceName: String, deviceIdHex: String) {
        if (listener != null) return
        val info = NsdServiceInfo().apply {
            serviceName = instanceName
            serviceType = SERVICE_TYPE
            port = Proto.PORT
            setAttribute("id", deviceIdHex)
            setAttribute("v", "1")
        }
        val l = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(info: NsdServiceInfo) { Elog.i("mDNS registered '${info.serviceName}' $SERVICE_TYPE") }
            override fun onRegistrationFailed(info: NsdServiceInfo, errorCode: Int) { Elog.w("mDNS registration failed: $errorCode") }
            override fun onServiceUnregistered(info: NsdServiceInfo) { Elog.i("mDNS unregistered") }
            override fun onUnregistrationFailed(info: NsdServiceInfo, errorCode: Int) { Elog.w("mDNS unregistration failed: $errorCode") }
        }
        listener = l
        try {
            nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, l)
        } catch (e: Exception) {
            Elog.w("mDNS register threw", e)
            listener = null
        }
    }

    fun stop() {
        val l = listener ?: return
        listener = null
        try { nsd.unregisterService(l) } catch (_: Exception) {}
    }

    companion object {
        const val SERVICE_TYPE = "_anydisplay._tcp"
    }
}
