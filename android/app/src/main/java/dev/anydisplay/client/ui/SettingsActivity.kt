package dev.anydisplay.client.ui

import android.content.Intent
import android.content.SharedPreferences
import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.preference.ListPreference
import androidx.preference.Preference
import androidx.preference.PreferenceFragmentCompat
import com.google.android.material.appbar.MaterialToolbar
import dev.anydisplay.client.R
import dev.anydisplay.client.data.Settings
import dev.anydisplay.client.device.DeviceCaps
import dev.anydisplay.client.session.SessionHub

class SettingsActivity : BaseActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_toolbar_frame)
        insetRoot(findViewById(R.id.root))
        findViewById<MaterialToolbar>(R.id.toolbar).apply {
            setTitle(R.string.settings)
            setNavigationOnClickListener { finish() }
        }
        if (savedInstanceState == null) {
            supportFragmentManager.beginTransaction().replace(R.id.content, SettingsFragment()).commit()
        }
    }

    class SettingsFragment : PreferenceFragmentCompat(), SharedPreferences.OnSharedPreferenceChangeListener {
        override fun onCreatePreferences(savedInstanceState: Bundle?, rootKey: String?) {
            setPreferencesFromResource(R.xml.settings, rootKey)
            val ctx = requireContext()
            findPreference<ListPreference>(Settings.K_REFRESH)?.let { p ->
                val choices = DeviceCaps.refreshChoices(ctx)
                p.entries = choices.map { "$it Hz" }.toTypedArray()
                p.entryValues = choices.map { it.toString() }.toTypedArray()
                if (p.value !in p.entryValues.map { it.toString() }) p.value = choices.last().coerceAtMost(60).toString()
            }
            findPreference<Preference>("paired")?.setOnPreferenceClickListener {
                startActivity(Intent(ctx, PairedHostsActivity::class.java)); true
            }
        }

        override fun onResume() {
            super.onResume()
            preferenceManager.sharedPreferences?.registerOnSharedPreferenceChangeListener(this)
        }

        override fun onPause() {
            preferenceManager.sharedPreferences?.unregisterOnSharedPreferenceChangeListener(this)
            super.onPause()
        }

        override fun onSharedPreferenceChanged(prefs: SharedPreferences?, key: String?) {
            // Stream settings take effect immediately for a running session.
            if (key in STREAM_KEYS) SessionHub.session?.refreshConfigRequest()
        }

        companion object {
            private val STREAM_KEYS = setOf(
                Settings.K_SCALE, Settings.K_REFRESH, Settings.K_CODEC, Settings.K_BITRATE, Settings.K_QUALITY,
                Settings.K_HIDPI, Settings.K_AUDIO, Settings.K_CLIENT_CURSOR, Settings.K_MIRROR, Settings.K_ORIENTATION,
            )
        }
    }
}

fun insetRoot(root: View) {
    ViewCompat.setOnApplyWindowInsetsListener(root) { v, insets ->
        val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
        insets
    }
}
