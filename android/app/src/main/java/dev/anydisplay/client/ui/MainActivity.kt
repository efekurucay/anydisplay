package dev.anydisplay.client.ui

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.View
import android.widget.Button
import android.widget.ImageView
import android.widget.TextView
import android.widget.Toast
import androidx.activity.enableEdgeToEdge
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.progressindicator.CircularProgressIndicator
import dev.anydisplay.client.BuildConfig
import dev.anydisplay.client.R
import dev.anydisplay.client.data.Identity
import dev.anydisplay.client.device.DeviceCaps
import dev.anydisplay.client.protocol.Proto
import dev.anydisplay.client.session.ConnectionService
import dev.anydisplay.client.session.Phase
import dev.anydisplay.client.session.SessionHub
import dev.anydisplay.client.session.Status

class MainActivity : BaseActivity() {
    private lateinit var statusIcon: ImageView
    private lateinit var statusProgress: CircularProgressIndicator
    private lateinit var statusTitle: TextView
    private lateinit var statusDetail: TextView
    private lateinit var statusError: TextView
    private lateinit var btnOpen: Button
    private lateinit var btnDisconnect: Button
    private lateinit var btnStart: Button
    private lateinit var wifiAddress: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        val root = findViewById<View>(R.id.root)
        ViewCompat.setOnApplyWindowInsetsListener(root) { v, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            insets
        }
        findViewById<MaterialToolbar>(R.id.toolbar).setOnMenuItemClickListener {
            when (it.itemId) {
                R.id.action_settings -> startActivity(Intent(this, SettingsActivity::class.java))
                R.id.action_paired -> startActivity(Intent(this, PairedHostsActivity::class.java))
            }
            true
        }
        statusIcon = findViewById(R.id.status_icon)
        statusProgress = findViewById(R.id.status_progress)
        statusTitle = findViewById(R.id.status_title)
        statusDetail = findViewById(R.id.status_detail)
        statusError = findViewById(R.id.status_error)
        btnOpen = findViewById(R.id.btn_open)
        btnDisconnect = findViewById(R.id.btn_disconnect)
        btnStart = findViewById(R.id.btn_start)
        wifiAddress = findViewById(R.id.wifi_address)
        btnOpen.setOnClickListener { startActivity(Intent(this, StreamActivity::class.java)) }
        btnDisconnect.setOnClickListener { SessionHub.session?.disconnect() }
        btnStart.setOnClickListener { ConnectionService.start(this) }
        findViewById<TextView>(R.id.usb_command).setOnLongClickListener { copy(getString(R.string.usb_command)); true }
        wifiAddress.setOnLongClickListener { copy(wifiAddress.text.toString()); true }
        findViewById<TextView>(R.id.footer).text =
            getString(R.string.device_footer, Identity(this).deviceIdHex.take(8), BuildConfig.VERSION_NAME)

        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
        ConnectionService.start(this)
    }

    override fun onResume() {
        super.onResume()
        val addresses = DeviceCaps.lanAddresses()
        wifiAddress.text = if (addresses.isEmpty()) getString(R.string.wifi_none) else addresses.joinToString("\n") { "$it:${Proto.PORT}" }
    }

    private fun copy(text: String) {
        (getSystemService(CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText("AnyDisplay", text))
        Toast.makeText(this, R.string.copied, Toast.LENGTH_SHORT).show()
    }

    override fun onStatus(status: Status) {
        val busy = status.phase == Phase.HANDSHAKE || status.phase == Phase.PAIRING
        statusProgress.visibility = if (busy || status.phase == Phase.LISTENING) View.VISIBLE else View.GONE
        statusProgress.isIndeterminate = true
        statusIcon.setImageResource(if (status.phase == Phase.CONNECTED) R.drawable.ic_link else R.drawable.ic_computer)
        when (status.phase) {
            Phase.STOPPED -> {
                statusTitle.setText(R.string.status_stopped)
                statusDetail.text = status.lastError ?: getString(R.string.status_stopped_detail)
            }
            Phase.LISTENING -> {
                statusTitle.setText(R.string.status_waiting)
                statusDetail.setText(R.string.status_waiting_detail)
            }
            Phase.HANDSHAKE -> {
                statusTitle.setText(R.string.status_handshake)
                statusDetail.text = status.peer
            }
            Phase.PAIRING -> {
                statusTitle.text = getString(R.string.status_pairing, status.hostName)
                statusDetail.setText(R.string.status_pairing_detail)
            }
            Phase.CONNECTED -> {
                statusTitle.text = getString(R.string.status_connected, status.hostName)
                val cfg = status.config
                val stream = if (cfg != null) " · ${cfg.width}×${cfg.height} ${Proto.codecName(cfg.codec)} ${cfg.refreshHz} Hz" else ""
                statusDetail.text = ConnectionService.linkLabel(this, status.link, status.mode) + stream
            }
        }
        val err = status.lastError
        statusError.visibility = if (err != null && status.phase == Phase.LISTENING) View.VISIBLE else View.GONE
        statusError.text = if (err != null) getString(R.string.last_error, err) else ""
        btnOpen.visibility = if (status.phase == Phase.CONNECTED) View.VISIBLE else View.GONE
        btnDisconnect.visibility = if (status.phase == Phase.CONNECTED) View.VISIBLE else View.GONE
        btnStart.visibility = if (status.phase == Phase.STOPPED) View.VISIBLE else View.GONE
    }
}
