package dev.anydisplay.client.protocol

import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.Closeable
import java.io.EOFException
import java.io.InputStream
import java.io.OutputStream
import java.util.ArrayDeque
import java.util.concurrent.atomic.AtomicLong

/** A byte stream to a host: TCP socket or USB accessory file descriptor. */
interface Transport : Closeable {
    val input: InputStream
    val output: OutputStream
    val link: Int
    val peer: String
}

/**
 * Frame reader/writer of PROTOCOL.md section 3. [send] is thread-safe; [receive] must be called from one thread.
 * Encryption is applied inside the write lock so the sequence numbers follow the wire order.
 */
class FrameChannel(private val transport: Transport) : Closeable {
    private val input = BufferedInputStream(transport.input, 256 * 1024)
    private val output = BufferedOutputStream(transport.output, 64 * 1024)
    private val writeLock = Any()
    private val header = ByteArray(6)
    private val outHeader = ByteArray(6)

    @Volatile private var sendCipher: FrameCipher? = null
    @Volatile private var recvCipher: FrameCipher? = null
    @Volatile var closed = false
        private set

    val bytesReceived = AtomicLong()
    val encrypted get() = sendCipher != null

    fun enableEncryption(sendKey: ByteArray, recvKey: ByteArray) {
        synchronized(writeLock) { sendCipher = FrameCipher(sendKey) }
        recvCipher = FrameCipher(recvKey)
    }

    fun send(channel: Int, type: Int, body: ByteArray = EMPTY, flush: Boolean = true) {
        synchronized(writeLock) {
            val payload = sendCipher?.encrypt(channel, type, body) ?: body
            val length = payload.size + 2
            outHeader[0] = length.toByte(); outHeader[1] = (length ushr 8).toByte()
            outHeader[2] = (length ushr 16).toByte(); outHeader[3] = (length ushr 24).toByte()
            outHeader[4] = channel.toByte(); outHeader[5] = type.toByte()
            output.write(outHeader)
            output.write(payload)
            if (flush) output.flush()
        }
    }

    fun flush() = synchronized(writeLock) { output.flush() }

    fun receive(): Frame {
        readFully(header, 6)
        val length = (header[0].toLong() and 0xFF) or ((header[1].toLong() and 0xFF) shl 8) or
            ((header[2].toLong() and 0xFF) shl 16) or ((header[3].toLong() and 0xFF) shl 24)
        if (length < 2 || length > Proto.MAX_FRAME) throw ProtocolException("bad frame length $length")
        val channel = header[4].toInt() and 0xFF
        val type = header[5].toInt() and 0xFF
        val body = ByteArray((length - 2).toInt())
        readFully(body, body.size)
        bytesReceived.addAndGet(length + 4)
        val plain = recvCipher?.decrypt(channel, type, body) ?: body
        return Frame(channel, type, plain)
    }

    private fun readFully(buf: ByteArray, n: Int) {
        var off = 0
        while (off < n) {
            val r = input.read(buf, off, n - off)
            if (r < 0) throw EOFException("connection closed")
            off += r
        }
    }

    override fun close() {
        closed = true
        try { transport.close() } catch (_: Exception) {}
    }

    companion object {
        val EMPTY = ByteArray(0)
    }
}

/**
 * Per-channel send queues drained in the priority order of PROTOCOL.md section 3:
 * control, input, cursor, audio, then video.
 */
class PrioritySender(private val channel: FrameChannel, private val onError: (Exception) -> Unit) {
    private val queues = Array(5) { ArrayDeque<Frame>() }
    private val lock = Object()
    private var running = true
    private var inFlight = false
    private val thread = Thread(::loop, "anydisplay-send")

    fun start() = thread.start()

    fun send(ch: Int, type: Int, body: ByteArray = FrameChannel.EMPTY) {
        synchronized(lock) {
            if (!running) return
            queues[ch.coerceIn(0, 4)].addLast(Frame(ch, type, body))
            lock.notifyAll()
        }
    }

    private fun next(): Frame? {
        for (ch in ORDER) queues[ch].pollFirst()?.let { return it }
        return null
    }

    private fun empty() = queues.all { it.isEmpty() }

    private fun loop() {
        try {
            while (true) {
                val f: Frame
                val more: Boolean
                synchronized(lock) {
                    while (running && empty()) lock.wait()
                    if (!running && empty()) return
                    f = next()!!
                    more = !empty()
                    inFlight = true
                }
                // Batch small frames: flush only when nothing else is waiting.
                channel.send(f.channel, f.type, f.body, flush = !more)
                synchronized(lock) { inFlight = false; lock.notifyAll() }
            }
        } catch (e: Exception) {
            synchronized(lock) { running = false; inFlight = false; lock.notifyAll() }
            if (!channel.closed) onError(e)
        }
    }

    /** Waits until queued frames are written, up to [timeoutMs]. */
    fun drain(timeoutMs: Long) {
        val end = System.currentTimeMillis() + timeoutMs
        synchronized(lock) {
            while ((!empty() || inFlight) && running) {
                val left = end - System.currentTimeMillis()
                if (left <= 0) return
                lock.wait(left)
            }
        }
    }

    fun stop() {
        synchronized(lock) {
            running = false
            for (q in queues) q.clear()
            lock.notifyAll()
        }
    }

    companion object {
        private val ORDER = intArrayOf(Proto.CH_CONTROL, Proto.CH_INPUT, Proto.CH_CURSOR, Proto.CH_AUDIO, Proto.CH_VIDEO)
    }
}

/** Calls [onExpire] once when an [arm]ed deadline passes without being re-armed (handshake and keepalive timeouts). */
class Watchdog(private val name: String, private val onExpire: (String) -> Unit) : Closeable {
    @Volatile private var deadlineNanos = Long.MAX_VALUE
    @Volatile private var reason = ""
    @Volatile private var stopped = false
    private val thread = Thread({
        while (!stopped) {
            try { Thread.sleep(100) } catch (_: InterruptedException) { return@Thread }
            if (System.nanoTime() > deadlineNanos && !stopped) {
                stopped = true
                onExpire(reason)
            }
        }
    }, "anydisplay-watchdog-$name").apply { isDaemon = true }

    fun start() = apply { thread.start() }

    fun arm(timeoutMs: Long, why: String) {
        reason = why
        deadlineNanos = System.nanoTime() + timeoutMs * 1_000_000
    }

    fun disarm() { deadlineNanos = Long.MAX_VALUE }

    override fun close() { stopped = true; thread.interrupt() }
}

/** TCP transport. A loopback peer means an adb-forwarded USB connection (link 0), anything else is network (link 1). */
class SocketTransport(private val socket: java.net.Socket) : Transport {
    init {
        socket.tcpNoDelay = true
    }

    override val input: InputStream = socket.getInputStream()
    override val output: OutputStream = socket.getOutputStream()
    override val link: Int = if (socket.inetAddress?.isLoopbackAddress == true) Proto.LINK_LOOPBACK else Proto.LINK_NETWORK
    override val peer: String = "${socket.inetAddress?.hostAddress}:${socket.port}"

    override fun close() {
        try { socket.shutdownOutput() } catch (_: Exception) {}
        socket.close()
    }
}
