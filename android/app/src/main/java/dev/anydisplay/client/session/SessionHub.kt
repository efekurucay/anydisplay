package dev.anydisplay.client.session

import android.os.Handler
import android.os.Looper
import dev.anydisplay.client.protocol.StreamConfig
import java.util.concurrent.CopyOnWriteArraySet

enum class Phase { STOPPED, LISTENING, HANDSHAKE, PAIRING, CONNECTED }

data class Status(
    val phase: Phase = Phase.STOPPED,
    val hostName: String = "",
    val hostOs: String = "",
    val mode: Int = -1,
    val link: Int = -1,
    val peer: String = "",
    val config: StreamConfig? = null,
    val lastError: String? = null,
)

class PairingRequest(val code: String, val hostName: String, val hostOs: String, private val decide: (Boolean) -> Unit) {
    @Volatile var decided = false
        private set

    fun answer(accept: Boolean) {
        if (decided) return
        decided = true
        decide(accept)
    }
}

/** Process-wide state shared by the connection service and the activities. Listeners run on the main thread. */
object SessionHub {
    interface Listener {
        fun onStatus(status: Status) {}
        fun onPairing(request: PairingRequest?) {}
        fun onNotice(code: Int, message: String) {}
    }

    private val main = Handler(Looper.getMainLooper())
    private val listeners = CopyOnWriteArraySet<Listener>()

    @Volatile var status = Status()
        private set

    @Volatile var pairing: PairingRequest? = null
        private set

    @Volatile var session: Session? = null

    /** Activities currently in the foreground; the service uses it to decide between a dialog and a notification. */
    @Volatile var foregroundActivities = 0

    fun addListener(l: Listener) {
        listeners += l
        l.onStatus(status)
        l.onPairing(pairing)
    }

    fun removeListener(l: Listener) {
        listeners -= l
    }

    fun update(transform: (Status) -> Status) {
        val s = synchronized(this) { transform(status).also { status = it } }
        main.post { listeners.forEach { it.onStatus(s) } }
    }

    fun setPairing(request: PairingRequest?) {
        pairing = request
        main.post { listeners.forEach { it.onPairing(request) } }
    }

    fun notice(code: Int, message: String) {
        main.post { listeners.forEach { it.onNotice(code, message) } }
    }
}
