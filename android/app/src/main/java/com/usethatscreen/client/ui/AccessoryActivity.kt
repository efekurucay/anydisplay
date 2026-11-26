package com.usethatscreen.client.ui

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import com.usethatscreen.client.session.ConnectionService

/** Receives USB_ACCESSORY_ATTACHED (which only activities can) and hands the accessory to the service. */
class AccessoryActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        ConnectionService.start(this, ConnectionService.ACTION_ATTACH_ACCESSORY, Intent(intent))
        finish()
    }
}
