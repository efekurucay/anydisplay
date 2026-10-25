package dev.anydisplay.client.session

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import dev.anydisplay.client.Elog
import dev.anydisplay.client.audio.AudioPlayer
import dev.anydisplay.client.data.Identity
import dev.anydisplay.client.data.PairedHosts
import dev.anydisplay.client.data.Settings
import dev.anydisplay.client.device.DeviceCaps
import dev.anydisplay.client.protocol.Bodies
import dev.anydisplay.client.protocol.ByteReader
import dev.anydisplay.client.protocol.ClientHandshake
import dev.anydisplay.client.protocol.ConfigReq
import dev.anydisplay.client.protocol.Frame
import dev.anydisplay.client.protocol.FrameChannel
import dev.anydisplay.client.protocol.HandshakeException
import dev.anydisplay.client.protocol.PrioritySender
import dev.anydisplay.client.protocol.Proto
import dev.anydisplay.client.protocol.SasPrompt
import dev.anydisplay.client.protocol.Stats
import dev.anydisplay.client.protocol.StreamConfig
import dev.anydisplay.client.protocol.Transport
import dev.anydisplay.client.protocol.VideoFrame
import dev.anydisplay.client.protocol.Watchdog
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

interface VideoSink {
    fun onConfig(config: StreamConfig)
    fun onFrame(frame: VideoFrame, receivedNanos: Long)
}

interface CursorSink {
    fun onCursorPos(x: Int, y: Int, visible: Boolean)
    fun onCursorShape(w: Int, h: Int, hotX: Int, hotY: Int, rgba: ByteArray, offset: Int)
}

/**
 * One host connection: handshake, then the session of PROTOCOL.md section 5.
 * The connection thread reads and dispatches frames; [PrioritySender] writes; a scheduler sends PING and STATS.
 */
class Session(
    context: Context,
    private val transport: Transport,
    private val onEnded: (Session) -> Unit,
) {
    private val app = context.applicationContext
    private val settings = Settings(app)
    private val main = Handler(Looper.getMainLooper())
    private val channel = FrameChannel(transport)
    private var sender: PrioritySender? = null
    private var scheduler: ScheduledExecutorService? = null
    private val audio = AudioPlayer()

    @Volatile private var endReason: String? = null
    @Volatile private var ended = false
    @Volatile var connected = false
        private set

    val link get() = transport.link
    @Volatile var hostName = ""
        private set
    @Volatile var hostOs = ""
        private set
    @Volatile var mode = -1
        private set
    @Volatile var config: StreamConfig? = null
        private set
    @Volatile var lastRequest: ConfigReq? = null
        private set

    val hostIsMac get() = hostOs.contains("mac", ignoreCase = true) || hostOs.contains("darwin", ignoreCase = true)

    // Counters for STATS (reset every second) and for the overlay (cumulative).
    private val framesDecoded = AtomicInteger()
    private val framesDropped = AtomicInteger()
    private val decodeUsSum = AtomicLong()
    val totalRendered = AtomicLong()
    val totalDropped = AtomicLong()
    val totalBytes = AtomicLong()
    /** Bytes received so far, for the overlay's bitrate. */
    fun bytesSoFar() = totalBytes.get() + channel.bytesReceived.get()
    @Volatile var rttUs = 0L
        private set
    @Volatile var lastAvgDecodeUs = 0L
        private set
    @Volatile var lastLatencyUs = -1L
        private set

    private val pingSeq = AtomicLong()
    private val baseEpochUs = System.currentTimeMillis() * 1000
    private val baseNanos = System.nanoTime()
    private fun epochUs() = baseEpochUs + (System.nanoTime() - baseNanos) / 1000

    // Host clock offset estimate from host PINGs: max(host_time - local_time) + minRtt / 2.
    private val offsetSamples = ArrayDeque<Long>()
    private var minRttUs = Long.MAX_VALUE
    @Volatile private var hostOffsetUs: Long? = null

    @Volatile private var videoSink: VideoSink? = null
    @Volatile private var cursorSink: CursorSink? = null
    @Volatile private var lastCursorShape: Frame? = null
    @Volatile private var lastCursorPos: Frame? = null
    @Volatile var lastClipboardFromHost: String? = null

    private val watchdog = Watchdog("session") { why ->
        Elog.w("watchdog: $why")
        endReason = "timed out $why"
        channel.close()
    }

    fun start() {
        Thread(::run, "anydisplay-conn").start()
    }

    private fun run() {
        watchdog.start()
        try {
            Elog.i("connection from ${transport.peer} link=${transport.link}")
            SessionHub.update { Status(Phase.HANDSHAKE, link = transport.link, peer = transport.peer) }
            val identity = Identity(app)
            val result = ClientHandshake(
                channel, identity.deviceId, identity.deviceName, transport.link, PairedHosts(app), prompt,
                setTimeout = { ms, why -> watchdog.arm(ms, why) },
                log = { Elog.i("handshake: $it") },
            ).run()
            hostName = result.hostName
            hostOs = result.hostOs
            mode = result.mode
            Elog.i("handshake ok: host='$hostName' os='$hostOs' mode=$mode link=${transport.link}")
            watchdog.arm(KEEPALIVE_MS, "waiting for host data")
            val s = PrioritySender(channel) { e -> fail("send failed: ${e.message}") }
            sender = s
            s.start()
            connected = true
            SessionHub.update {
                it.copy(phase = Phase.CONNECTED, hostName = hostName, hostOs = hostOs, mode = mode, lastError = null)
            }
            send(Proto.CH_CONTROL, Proto.DEVICE_INFO, DeviceCaps.deviceInfo(app, settings).also { Elog.i("send $it") }.encode())
            sendConfigRequest()
            scheduler = Executors.newSingleThreadScheduledExecutor { r -> Thread(r, "anydisplay-tick") }.apply {
                scheduleWithFixedDelay(::tick, 1, 1, TimeUnit.SECONDS)
            }
            while (!ended) dispatch(channel.receive())
        } catch (e: HandshakeException) {
            endReason = endReason ?: e.message
            Elog.w("handshake failed: ${e.message}")
        } catch (e: Exception) {
            if (endReason == null && !ended) endReason = e.message ?: e.javaClass.simpleName
            Elog.i("connection closed: ${endReason ?: e}")
        } finally {
            finish()
        }
    }

    private val prompt = object : SasPrompt {
        override fun show(code: String, hostName: String, hostOs: String, decide: (Boolean) -> Unit) {
            SessionHub.update { it.copy(phase = Phase.PAIRING, hostName = hostName, hostOs = hostOs) }
            SessionHub.setPairing(PairingRequest(code, hostName, hostOs, decide))
        }

        override fun dismiss() {
            SessionHub.setPairing(null)
        }
    }

    // ------------------------------------------------------------ receive

    private fun dispatch(f: Frame) {
        watchdog.arm(KEEPALIVE_MS, "(no data from host for ${KEEPALIVE_MS / 1000} s)")
        when (f.channel) {
            Proto.CH_VIDEO -> if (f.type == Proto.VIDEO_FRAME) onVideo(f)
            Proto.CH_CONTROL -> onControl(f)
            Proto.CH_AUDIO -> onAudio(f)
            Proto.CH_CURSOR -> onCursor(f)
        }
    }

    private fun onControl(f: Frame) {
        val r = ByteReader(f.body)
        when (f.type) {
            Proto.PING -> {
                if (f.body.size >= 12) {
                    send(Proto.CH_CONTROL, Proto.PONG, f.body.copyOf(12))
                    r.u32()
                    sampleHostClock(r.u64())
                }
            }
            Proto.PONG -> {
                r.u32()
                val rtt = SystemClock.elapsedRealtimeNanos() / 1000 - r.u64()
                if (rtt in 0..10_000_000) {
                    rttUs = rtt
                    minRttUs = minOf(minRttUs, rtt)
                }
            }
            Proto.CONFIG -> {
                val c = StreamConfig.decode(f.body)
                config = c
                Elog.i("CONFIG $c")
                SessionHub.update { it.copy(config = c) }
                videoSink?.onConfig(c)
                if (!c.audio) audio.release()
                if (!c.clientCursor) { lastCursorPos = null; lastCursorShape = null }
            }
            Proto.NOTICE -> {
                val code = r.u16()
                val msg = r.str()
                Elog.i("NOTICE $code $msg")
                SessionHub.notice(code, msg)
            }
            Proto.CLIPBOARD -> {
                val mime = r.str()
                val data = r.blob()
                if (mime == Proto.MIME_TEXT) {
                    val text = String(data, Charsets.UTF_8)
                    lastClipboardFromHost = text
                    Elog.i("clipboard from host: ${text.length} chars")
                    main.post {
                        val cm = app.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                        cm.setPrimaryClip(ClipData.newPlainText("AnyDisplay", text))
                    }
                }
            }
            Proto.BYE -> {
                val reason = if (f.body.isNotEmpty()) f.body[0].toInt() else 0
                endReason = "host closed the session" + if (reason == Proto.BYE_ERROR) " (error)" else ""
                ended = true
                channel.close()
            }
            Proto.AUTH_FAIL -> {
                endReason = "host sent AUTH_FAIL(${Proto.failReasonName(f.body.firstOrNull()?.toInt() ?: 0)})"
                ended = true
                channel.close()
            }
        }
    }

    private fun sampleHostClock(hostUs: Long) {
        synchronized(offsetSamples) {
            offsetSamples.addLast(hostUs - epochUs())
            while (offsetSamples.size > 16) offsetSamples.removeFirst()
            val rtt = if (minRttUs == Long.MAX_VALUE) 0 else minRttUs
            hostOffsetUs = offsetSamples.max() + rtt / 2
        }
    }

    private fun onVideo(f: Frame) {
        val vf = try { VideoFrame.decode(f.body) } catch (e: Exception) { Elog.w("bad VIDEO_FRAME: ${e.message}"); return }
        val sink = videoSink
        if (sink == null || config == null) {
            ack(vf.streamId, vf.frameId, 0, rendered = false)
            return
        }
        sink.onFrame(vf, System.nanoTime())
    }

    private fun onAudio(f: Frame) {
        if (!settings.audio || config?.audio != true) return
        val r = ByteReader(f.body)
        when (f.type) {
            Proto.AUDIO_CONFIG -> audio.configure(r.u32().toInt(), r.u8(), r.u8())
            Proto.AUDIO_DATA -> if (f.body.size > 8) audio.write(f.body, 8, f.body.size - 8)
        }
    }

    private fun onCursor(f: Frame) {
        if (config?.clientCursor != true) return
        val sink = cursorSink
        when (f.type) {
            Proto.CURSOR_POS -> {
                lastCursorPos = f
                if (sink != null) deliverPos(sink, f)
            }
            Proto.CURSOR_SHAPE -> {
                lastCursorShape = f
                if (sink != null) deliverShape(sink, f)
            }
        }
    }

    private fun deliverPos(sink: CursorSink, f: Frame) {
        val r = ByteReader(f.body)
        sink.onCursorPos(r.i16(), r.i16(), r.u8() != 0)
    }

    private fun deliverShape(sink: CursorSink, f: Frame) {
        val r = ByteReader(f.body)
        val w = r.u16(); val h = r.u16(); val hx = r.u16(); val hy = r.u16()
        if (w <= 0 || h <= 0 || w > 512 || h > 512 || f.body.size < 8 + w * h * 4) return
        sink.onCursorShape(w, h, hx, hy, f.body, 8)
    }

    // ------------------------------------------------------------ periodic

    private fun tick() {
        try {
            send(Proto.CH_CONTROL, Proto.PING, Bodies.ping(pingSeq.getAndIncrement(), SystemClock.elapsedRealtimeNanos() / 1000))
            val decoded = framesDecoded.getAndSet(0)
            val dropped = framesDropped.getAndSet(0)
            val decodeSum = decodeUsSum.getAndSet(0)
            val avg = if (decoded > 0) decodeSum / decoded else 0
            lastAvgDecodeUs = avg
            val bytes = channel.bytesReceived.getAndSet(0)
            totalBytes.addAndGet(bytes)
            send(Proto.CH_CONTROL, Proto.STATS, Stats(decoded, dropped, avg.toInt(), rttUs.toInt(), bytes).encode())
        } catch (e: Exception) {
            Elog.w("tick failed", e)
        }
    }

    // ------------------------------------------------------------ API for the UI and decoder

    fun send(ch: Int, type: Int, body: ByteArray = FrameChannel.EMPTY) {
        sender?.send(ch, type, body)
    }

    fun sendInput(type: Int, body: ByteArray) {
        if (connected) send(Proto.CH_INPUT, type, body)
    }

    fun sendConfigRequest() {
        val req = DeviceCaps.configRequest(app, settings)
        lastRequest = req
        Elog.i("send $req")
        send(Proto.CH_CONTROL, Proto.CONFIG_REQ, req.encode())
    }

    /** Re-sends CONFIG_REQ only when the requested stream would change (rotation, settings). */
    fun refreshConfigRequest() {
        val req = DeviceCaps.configRequest(app, settings)
        if (req != lastRequest) sendConfigRequest()
    }

    fun requestKeyframe() = send(Proto.CH_CONTROL, Proto.KEYFRAME_REQ)

    fun sendClipboard(text: String) {
        if (!connected || text == lastClipboardFromHost) return
        lastClipboardFromHost = text
        send(Proto.CH_CONTROL, Proto.CLIPBOARD, Bodies.clipboard(Proto.MIME_TEXT, text.toByteArray(Charsets.UTF_8)))
    }

    /** FRAME_ACK after render or drop (PROTOCOL.md 5.2). */
    fun ack(streamId: Long, frameId: Long, decodeUs: Long, rendered: Boolean, hostPtsUs: Long = 0) {
        send(Proto.CH_VIDEO, Proto.FRAME_ACK, Bodies.frameAck(streamId, frameId, decodeUs))
        if (rendered) {
            framesDecoded.incrementAndGet()
            decodeUsSum.addAndGet(decodeUs)
            totalRendered.incrementAndGet()
            val off = hostOffsetUs
            if (off != null && hostPtsUs > 0) {
                val lat = epochUs() + off - hostPtsUs
                lastLatencyUs = if (lat in 0..2_000_000) lat else -1
            }
        } else {
            framesDropped.incrementAndGet()
            totalDropped.incrementAndGet()
        }
    }

    fun attachVideo(sink: VideoSink?) {
        videoSink = sink
        if (sink != null) {
            config?.let { sink.onConfig(it) }
            requestKeyframe()
        }
    }

    fun detachVideo(sink: VideoSink) {
        if (videoSink === sink) videoSink = null
    }

    fun attachCursor(sink: CursorSink?) {
        cursorSink = sink
        if (sink != null) {
            lastCursorShape?.let { deliverShape(sink, it) }
            lastCursorPos?.let { deliverPos(sink, it) }
        }
    }

    fun disconnect(reason: Int = Proto.BYE_USER) {
        if (ended) return
        endReason = "disconnected"
        val s = sender
        if (s != null && connected) {
            s.send(Proto.CH_CONTROL, Proto.BYE, Bodies.u8(reason))
            Thread {
                s.drain(500)
                ended = true
                channel.close()
            }.start()
        } else {
            ended = true
            channel.close()
        }
    }

    private fun fail(why: String) {
        if (endReason == null) endReason = why
        channel.close()
    }

    private fun finish() {
        ended = true
        connected = false
        watchdog.close()
        scheduler?.shutdownNow()
        sender?.stop()
        channel.close()
        audio.release()
        SessionHub.setPairing(null)
        val reason = endReason
        Elog.i("session ended: $reason")
        onEnded(this)
    }

    val endMessage: String? get() = endReason

    companion object {
        const val KEEPALIVE_MS = 5_000L

        /** Answers a connection that arrives while a session is active, then closes it. */
        fun rejectBusy(transport: Transport) {
            Thread({
                val ch = FrameChannel(transport)
                try {
                    ch.send(Proto.CH_CONTROL, Proto.AUTH_FAIL, Bodies.u8(Proto.FAIL_BUSY))
                    // Drain what the host sends until it closes, so closing does not reset the connection
                    // before the host has read AUTH_FAIL.
                    Watchdog("busy") { ch.close() }.start().arm(2_000, "busy")
                    while (true) ch.receive()
                } catch (_: Exception) {
                } finally {
                    ch.close()
                }
            }, "anydisplay-busy").start()
        }
    }
}
