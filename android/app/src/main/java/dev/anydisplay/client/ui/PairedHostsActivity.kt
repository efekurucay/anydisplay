package dev.anydisplay.client.ui

import android.os.Bundle
import android.text.format.DateUtils
import android.view.Gravity
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.FrameLayout
import android.widget.TextView
import androidx.activity.enableEdgeToEdge
import androidx.recyclerview.widget.DividerItemDecoration
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import dev.anydisplay.client.R
import dev.anydisplay.client.data.PairedHost
import dev.anydisplay.client.data.PairedHosts

class PairedHostsActivity : BaseActivity() {
    private lateinit var store: PairedHosts
    private lateinit var list: RecyclerView
    private lateinit var empty: TextView
    private var items: List<PairedHost> = emptyList()

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_toolbar_frame)
        insetRoot(findViewById(R.id.root))
        findViewById<MaterialToolbar>(R.id.toolbar).apply {
            setTitle(R.string.paired_computers)
            setNavigationOnClickListener { finish() }
        }
        store = PairedHosts(this)
        val content = findViewById<FrameLayout>(R.id.content)
        list = RecyclerView(this).apply {
            layoutManager = LinearLayoutManager(this@PairedHostsActivity)
            addItemDecoration(DividerItemDecoration(this@PairedHostsActivity, DividerItemDecoration.VERTICAL))
            adapter = Adapter()
        }
        empty = TextView(this).apply {
            setText(R.string.paired_empty)
            gravity = Gravity.CENTER
            val pad = (32 * resources.displayMetrics.density).toInt()
            setPadding(pad, pad, pad, pad)
            setTextAppearance(com.google.android.material.R.style.TextAppearance_Material3_BodyLarge)
        }
        content.addView(list)
        content.addView(empty)
        reload()
    }

    private fun reload() {
        items = store.list()
        list.adapter?.notifyDataSetChanged()
        empty.visibility = if (items.isEmpty()) View.VISIBLE else View.GONE
    }

    private fun relative(ms: Long): CharSequence = when {
        ms <= 0 -> "—"
        System.currentTimeMillis() - ms < DateUtils.MINUTE_IN_MILLIS -> getString(R.string.just_now)
        else -> DateUtils.getRelativeTimeSpanString(ms, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS)
    }

    private inner class Holder(v: View) : RecyclerView.ViewHolder(v) {
        val name: TextView = v.findViewById(R.id.name)
        val detail: TextView = v.findViewById(R.id.detail)
        val forget: Button = v.findViewById(R.id.forget)
    }

    private inner class Adapter : RecyclerView.Adapter<Holder>() {
        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int) =
            Holder(LayoutInflater.from(parent.context).inflate(R.layout.item_paired_host, parent, false))

        override fun getItemCount() = items.size

        override fun onBindViewHolder(h: Holder, position: Int) {
            val host = items[position]
            h.name.text = host.name.ifBlank { host.hostIdHex.take(8) }
            h.detail.text = getString(R.string.paired_detail, host.os.ifBlank { "?" }, relative(host.pairedAtMs), relative(host.lastSeenMs))
            h.forget.setOnClickListener {
                MaterialAlertDialogBuilder(this@PairedHostsActivity)
                    .setTitle(getString(R.string.forget_title, h.name.text))
                    .setMessage(R.string.forget_message)
                    .setNegativeButton(R.string.cancel, null)
                    .setPositiveButton(R.string.forget) { _, _ ->
                        store.forget(host.hostIdHex)
                        reload()
                    }
                    .show()
            }
        }
    }
}
