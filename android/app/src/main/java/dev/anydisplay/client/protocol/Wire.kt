package dev.anydisplay.client.protocol

/** Little-endian body builder for PROTOCOL.md section 1 types. */
class ByteWriter(initial: Int = 64) {
    private var buf = ByteArray(initial)
    var size = 0
        private set

    private fun ensure(extra: Int) {
        if (size + extra > buf.size) buf = buf.copyOf(maxOf(buf.size * 2, size + extra))
    }

    fun u8(v: Int) = apply { ensure(1); buf[size++] = v.toByte() }

    fun u16(v: Int) = apply {
        ensure(2)
        buf[size++] = v.toByte(); buf[size++] = (v ushr 8).toByte()
    }

    fun i16(v: Int) = u16(v)

    fun u32(v: Long) = apply {
        ensure(4)
        for (i in 0 until 4) buf[size++] = (v ushr (8 * i)).toByte()
    }

    fun u32(v: Int) = u32(v.toLong() and 0xFFFFFFFFL)

    fun u64(v: Long) = apply {
        ensure(8)
        for (i in 0 until 8) buf[size++] = (v ushr (8 * i)).toByte()
    }

    fun f32(v: Float) = u32(java.lang.Float.floatToIntBits(v))

    fun raw(b: ByteArray) = apply { ensure(b.size); System.arraycopy(b, 0, buf, size, b.size); size += b.size }

    fun str(s: String) = apply {
        val e = s.toByteArray(Charsets.UTF_8)
        require(e.size <= 0xFFFF) { "string too long" }
        u16(e.size).raw(e)
    }

    fun blob(b: ByteArray) = u32(b.size).raw(b)

    fun bytes(): ByteArray = buf.copyOf(size)
}

class ProtocolException(message: String) : Exception(message)

/** Little-endian body parser. Throws [ProtocolException] on truncated input. */
class ByteReader(private val b: ByteArray, private var o: Int = 0, private val end: Int = b.size) {
    val remaining get() = end - o
    val offset get() = o

    private fun need(n: Int) {
        if (n < 0 || o + n > end) throw ProtocolException("short body: need $n, have ${end - o}")
    }

    fun u8(): Int { need(1); return b[o++].toInt() and 0xFF }

    fun u16(): Int {
        need(2)
        val v = (b[o].toInt() and 0xFF) or ((b[o + 1].toInt() and 0xFF) shl 8)
        o += 2
        return v
    }

    fun i16(): Int = u16().toShort().toInt()

    fun u32(): Long {
        need(4)
        var v = 0L
        for (i in 0 until 4) v = v or ((b[o + i].toLong() and 0xFF) shl (8 * i))
        o += 4
        return v
    }

    fun u32i(): Int = u32().toInt()

    fun u64(): Long {
        need(8)
        var v = 0L
        for (i in 0 until 8) v = v or ((b[o + i].toLong() and 0xFF) shl (8 * i))
        o += 8
        return v
    }

    fun f32(): Float = java.lang.Float.intBitsToFloat(u32i())

    fun raw(n: Int): ByteArray { need(n); val v = b.copyOfRange(o, o + n); o += n; return v }

    fun str(): String { val n = u16(); need(n); val s = String(b, o, n, Charsets.UTF_8); o += n; return s }

    fun blob(): ByteArray {
        val n = u32()
        if (n > Int.MAX_VALUE) throw ProtocolException("blob too large")
        return raw(n.toInt())
    }

    fun rest(): ByteArray { val v = b.copyOfRange(o, end); o = end; return v }
}

fun ByteArray.toHex(): String {
    val sb = StringBuilder(size * 2)
    for (x in this) {
        val v = x.toInt() and 0xFF
        sb.append("0123456789abcdef"[v ushr 4]).append("0123456789abcdef"[v and 15])
    }
    return sb.toString()
}

fun String.hexToBytes(): ByteArray {
    require(length % 2 == 0) { "odd hex length" }
    return ByteArray(length / 2) { i -> substring(2 * i, 2 * i + 2).toInt(16).toByte() }
}
