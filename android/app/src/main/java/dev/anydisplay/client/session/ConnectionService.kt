package dev.anydisplay.client.session

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import dev.anydisplay.client.Elog
import dev.anydisplay.client.R
import dev.anydisplay.client.data.Identity
import dev.anydisplay.client.data.Settings
import dev.anydisplay.client.device.DeviceCaps
import dev.anydisplay.client.protocol.Proto
import dev.anydisplay.client.protocol.SocketTransport
import dev.anydisplay.client.protocol.Transport
import dev.anydisplay.client.ui.MainActivity
import dev.anydisplay.client.ui.StreamActivity
import java.net.InetSocketAddress
import java.net.ServerSocket

/**
 * Foreground service that owns the TCP listener on 47820, the mDNS advertisement, USB accessory links, and the one
 * active [Session], so a connection survives the activities going to the background.
 */
class ConnectionService : Service(), SessionHub.Listener {
    private var server: ServerSocket? = null
    private var acceptThread: Thread? = null
    private lateinit var nsd: NsdAdvertiser
    @Volatile private var stopping = false
    private val lock = Any()

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        createChannels(this)
        goForeground()
        nsd = NsdAdvertiser(this)
        val identity = Identity(this)
        nsd.start(identity.deviceName, identity.deviceIdHex)
        startListening()
        SessionHub.addListener(this)
        checkAttachedAccessory()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopping = true
                SessionHub.session?.disconnect(Proto.BYE_SHUTDOWN)
                stopSelf()
            }
            ACTION_DISCONNECT -> SessionHub.session?.disconnect()
            ACTION_ACCEPT_PAIRING -> SessionHub.pairing?.answer(true)
            ACTION_REJECT_PAIRING -> SessionHub.pairing?.answer(false)
            ACTION_ATTACH_ACCESSORY -> {
                @Suppress("DEPRECATION")
                val acc: UsbAccessory? = if (Build.VERSION.SDK_INT >= 33) {
                    intent.getParcelableExtra(UsbManager.EXTRA_ACCESSORY, UsbAccessory::class.java)
                } else intent.getParcelableExtra(UsbManager.EXTRA_ACCESSORY)
                acc?.let { openAccessory(it) }
            }
        }
        return START_STICKY
    }

    private fun goForeground() {
        val n = buildNotification(SessionHub.status)
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIF_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        } else {
            startForeground(NOTIF_ID, n)
        }
    }

    private fun startListening() {
        val ss = try {
            ServerSocket().apply {
                reuseAddress = true
                receiveBufferSize = 4 * 1024 * 1024
                bind(InetSocketAddress(Proto.PORT))
            }
        } catch (e: Exception) {
            Elog.e("cannot listen on ${Proto.PORT}", e)
            SessionHub.update { it.copy(phase = Phase.STOPPED, lastError = "Port ${Proto.PORT} is in use") }
            return
        }
        server = ss
        SessionHub.update { it.copy(phase = Phase.LISTENING) }
        Elog.i("listening on ${Proto.PORT}")
        acceptThread = Thread({
            while (!stopping) {
                try {
                    val sock = ss.accept()
                    sock.receiveBufferSize = 4 * 1024 * 1024
                    sock.sendBufferSize = 512 * 1024
                    begin(SocketTransport(sock))
                } catch (e: Exception) {
                    if (!stopping) Elog.w("accept failed: ${e.message}")
                    if (ss.isClosed) break
                }
            }
        }, "anydisplay-accept").apply { start() }
    }

    private fun begin(transport: Transport) {
        synchronized(lock) {
            if (SessionHub.session != null) {
                Elog.i("busy: rejecting ${transport.peer}")
                Session.rejectBusy(transport)
                return
            }
            val s = Session(this, transport, ::onSessionEnded)
            SessionHub.session = s
            s.start()
        }
    }

    private fun onSessionEnded(s: Session) {
        synchronized(lock) {
            if (SessionHub.session === s) SessionHub.session = null
        }
        SessionHub.update {
            Status(phase = if (server != null && !stopping) Phase.LISTENING else Phase.STOPPED, lastError = s.endMessage)
        }
    }

    private fun checkAttachedAccessory() {
        val usb = getSystemService(Context.USB_SERVICE) as UsbManager
        usb.accessoryList?.firstOrNull { it.manufacturer == "AnyDisplay" && usb.hasPermission(it) }?.let { openAccessory(it) }
    }

    private fun openAccessory(acc: UsbAccessory) {
        val usb = getSystemService(Context.USB_SERVICE) as UsbManager
        val pfd = try { usb.openAccessory(acc) } catch (e: Exception) { Elog.w("openAccessory failed", e); null }
        if (pfd == null) {
            Elog.w("cannot open accessory ${acc.model}")
            return
        }
        Elog.i("USB accessory opened: ${acc.manufacturer} ${acc.model}")
        begin(AccessoryTransport(pfd, acc.model ?: ""))
    }

    // ------------------------------------------------------------ hub events

    private var lastPhase: Phase? = null

    override fun onStatus(status: Status) {
        notificationManager().notify(NOTIF_ID, buildNotification(status))
        if (status.phase == Phase.CONNECTED && lastPhase != Phase.CONNECTED && Settings(this).autoStart) {
            // Allowed while one of our activities is visible; otherwise the notification opens the display.
            try {
                startActivity(Intent(this, StreamActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            } catch (e: Exception) {
                Elog.w("cannot start stream activity: ${e.message}")
            }
        }
        lastPhase = status.phase
    }

    override fun onPairing(request: PairingRequest?) {
        val nm = notificationManager()
        if (request == null) {
            nm.cancel(PAIRING_NOTIF_ID)
            return
        }
        if (SessionHub.foregroundActivities > 0) return // the visible activity shows the dialog
        val open = PendingIntent.getActivity(this, 1, Intent(this, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP), PendingIntent.FLAG_IMMUTABLE)
        val n = NotificationCompat.Builder(this, CHANNEL_PAIRING)
            .setSmallIcon(R.drawable.ic_stat_anydisplay)
            .setContentTitle(getString(R.string.pairing_notif_title, request.hostName))
            .setContentText(getString(R.string.pairing_notif_text, request.code))
            .setStyle(NotificationCompat.BigTextStyle().bigText(getString(R.string.pairing_notif_text, request.code)))
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setContentIntent(open)
            .setFullScreenIntent(open, true)
            .setAutoCancel(true)
            .addAction(0, getString(R.string.reject), servicePending(ACTION_REJECT_PAIRING, 3))
            .addAction(0, getString(R.string.accept), servicePending(ACTION_ACCEPT_PAIRING, 2))
            .build()
        nm.notify(PAIRING_NOTIF_ID, n)
    }

    override fun onNotice(code: Int, message: String) {
        if (SessionHub.foregroundActivities == 0) {
            android.widget.Toast.makeText(this, message, android.widget.Toast.LENGTH_LONG).show()
        }
    }

    // ------------------------------------------------------------ notification

    private fun notificationManager() = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager

    private fun servicePending(action: String, code: Int) = PendingIntent.getService(
        this, code, Intent(this, ConnectionService::class.java).setAction(action), PendingIntent.FLAG_IMMUTABLE,
    )

    private fun buildNotification(status: Status): Notification {
        val address = DeviceCaps.lanAddresses().firstOrNull()
        val (title, text) = when (status.phase) {
            Phase.CONNECTED -> getString(R.string.notif_connected, status.hostName) to linkLabel(this, status.link, status.mode)
            Phase.PAIRING -> getString(R.string.notif_pairing, status.hostName) to getString(R.string.notif_pairing_text)
            Phase.HANDSHAKE -> getString(R.string.notif_handshake) to status.peer
            else -> getString(R.string.notif_waiting) to
                (if (address != null) "$address:${Proto.PORT}" else getString(R.string.notif_usb_only))
        }
        val connected = status.phase == Phase.CONNECTED
        val tap = PendingIntent.getActivity(
            this, 0,
            Intent(this, if (connected) StreamActivity::class.java else MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_SERVICE)
            .setSmallIcon(R.drawable.ic_stat_anydisplay)
            .setContentTitle(title)
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setContentIntent(tap)
            .apply {
                if (connected) addAction(0, getString(R.string.disconnect), servicePending(ACTION_DISCONNECT, 4))
                addAction(0, getString(R.string.stop_service), servicePending(ACTION_STOP, 5))
            }
            .build()
    }

    override fun onDestroy() {
        stopping = true
        SessionHub.removeListener(this)
        nsd.stop()
        try { server?.close() } catch (_: Exception) {}
        server = null
        SessionHub.session?.disconnect(Proto.BYE_SHUTDOWN)
        SessionHub.update { Status(phase = Phase.STOPPED) }
        super.onDestroy()
    }

    companion object {
        const val ACTION_STOP = "dev.anydisplay.client.STOP"
        const val ACTION_DISCONNECT = "dev.anydisplay.client.DISCONNECT"
        const val ACTION_ACCEPT_PAIRING = "dev.anydisplay.client.ACCEPT_PAIRING"
        const val ACTION_REJECT_PAIRING = "dev.anydisplay.client.REJECT_PAIRING"
        const val ACTION_ATTACH_ACCESSORY = "dev.anydisplay.client.ATTACH_ACCESSORY"
        private const val CHANNEL_SERVICE = "connection"
        private const val CHANNEL_PAIRING = "pairing"
        private const val NOTIF_ID = 1
        private const val PAIRING_NOTIF_ID = 2

        fun start(context: Context, action: String? = null, extras: Intent? = null) {
            val i = Intent(context, ConnectionService::class.java).setAction(action)
            extras?.extras?.let { i.putExtras(it) }
            ContextCompat.startForegroundService(context, i)
        }

        fun stop(context: Context) = start(context, ACTION_STOP)

        fun createChannels(context: Context) {
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            nm.createNotificationChannel(NotificationChannel(CHANNEL_SERVICE, context.getString(R.string.channel_connection), NotificationManager.IMPORTANCE_LOW))
            nm.createNotificationChannel(NotificationChannel(CHANNEL_PAIRING, context.getString(R.string.channel_pairing), NotificationManager.IMPORTANCE_HIGH))
        }

        fun linkLabel(context: Context, link: Int, mode: Int): String {
            val l = when (link) {
                Proto.LINK_LOOPBACK -> context.getString(R.string.link_usb_adb)
                Proto.LINK_USB_ACCESSORY -> context.getString(R.string.link_usb_accessory)
                else -> context.getString(R.string.link_wifi)
            }
            val m = if (mode == Proto.MODE_NONE) context.getString(R.string.mode_plain) else context.getString(R.string.mode_encrypted)
            return "$l · $m"
        }
    }
}
