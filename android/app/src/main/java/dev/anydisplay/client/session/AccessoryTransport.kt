package dev.anydisplay.client.session

import android.os.ParcelFileDescriptor
import dev.anydisplay.client.protocol.Proto
import dev.anydisplay.client.protocol.Transport
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.InputStream
import java.io.OutputStream

/** USB accessory (AOAP) link: the same protocol over the accessory's bulk endpoints (link 2). */
class AccessoryTransport(private val pfd: ParcelFileDescriptor, name: String) : Transport {
    override val input: InputStream = PacketInput(FileInputStream(pfd.fileDescriptor))
    override val output: OutputStream = FileOutputStream(pfd.fileDescriptor)
    override val link = Proto.LINK_USB_ACCESSORY
    override val peer = "USB accessory $name"

    override fun close() {
        try { pfd.close() } catch (_: Exception) {}
    }

    /** Accessory reads must use a buffer of at least one USB transfer, so always read into a 16 KiB chunk. */
    private class PacketInput(private val raw: InputStream) : InputStream() {
        private val buf = ByteArray(16384)
        private var pos = 0
        private var len = 0

        private fun fill(): Boolean {
            while (pos >= len) {
                val n = raw.read(buf, 0, buf.size)
                if (n < 0) return false
                pos = 0
                len = n
            }
            return true
        }

        override fun read(): Int = if (fill()) buf[pos++].toInt() and 0xFF else -1

        override fun read(b: ByteArray, off: Int, n: Int): Int {
            if (n == 0) return 0
            if (!fill()) return -1
            val k = minOf(n, len - pos)
            System.arraycopy(buf, pos, b, off, k)
            pos += k
            return k
        }

        override fun available() = len - pos
        override fun close() = raw.close()
    }
}
