package dev.anydisplay.client.ui

import android.os.Build
import android.os.Bundle
import android.view.LayoutInflater
import android.widget.TextView
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.snackbar.Snackbar
import dev.anydisplay.client.BuildConfig
import dev.anydisplay.client.R
import dev.anydisplay.client.session.PairingRequest
import dev.anydisplay.client.session.SessionHub
import dev.anydisplay.client.session.Status

/** Shows the pairing dialog and host notices on whichever AnyDisplay screen is in front. */
abstract class BaseActivity : AppCompatActivity(), SessionHub.Listener {
    private var pairingDialog: AlertDialog? = null
    private var shownRequest: PairingRequest? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Debug builds only: lets automated device tests drive the UI while a secure lock screen is up.
        if (BuildConfig.DEBUG && Build.VERSION.SDK_INT >= 27) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
            window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }

    override fun onResume() {
        super.onResume()
        SessionHub.foregroundActivities++
        SessionHub.addListener(this)
    }

    override fun onPause() {
        SessionHub.removeListener(this)
        SessionHub.foregroundActivities--
        super.onPause()
    }

    override fun onStatus(status: Status) {}

    override fun onPairing(request: PairingRequest?) {
        if (request === shownRequest) return
        pairingDialog?.dismiss()
        pairingDialog = null
        shownRequest = request
        if (request == null || request.decided) return
        val view = LayoutInflater.from(this).inflate(R.layout.dialog_pairing, null)
        view.findViewById<TextView>(R.id.pairing_message).text = getString(R.string.pairing_message, request.hostName)
        view.findViewById<TextView>(R.id.pairing_code).apply {
            text = request.code.substring(0, 3) + " " + request.code.substring(3)
            contentDescription = request.code
        }
        pairingDialog = MaterialAlertDialogBuilder(this)
            .setTitle(getString(R.string.pairing_title, request.hostName))
            .setView(view)
            .setCancelable(false)
            .setNegativeButton(R.string.reject) { _, _ -> request.answer(false) }
            .setPositiveButton(R.string.accept) { _, _ -> request.answer(true) }
            .show()
    }

    override fun onNotice(code: Int, message: String) {
        Snackbar.make(findViewById(android.R.id.content), message, Snackbar.LENGTH_LONG).show()
    }

    override fun onDestroy() {
        pairingDialog?.dismiss()
        super.onDestroy()
    }
}
