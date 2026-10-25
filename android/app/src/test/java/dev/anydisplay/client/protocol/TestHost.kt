package dev.anydisplay.client.protocol

import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket

/** A Kotlin port of the host role of tools/anydisplay_ref.py (host_handshake), for in-process tests. */
class TestHost(
    val hostId: ByteArray = Crypto.randomBytes(16),
    val secrets: MutableMap<String, ByteArray> = HashMap(),
    var acceptSas: Boolean = true,
    var hostOs: String = "macOS",
) {
    var sasShown: String? = null
    var lastMode = -1

    class Result(val channel: FrameChannel, val mode: Int, val deviceName: String)

    fun handshake(channel: FrameChannel): Result {
        val helloC = expect(channel, Proto.HELLO_C)
        val r = ByteReader(helloC)
        check(r.raw(4).contentEquals(Proto.MAGIC) && r.u16() == 1)
        r.u16()
        val deviceId = r.raw(16).toHex()
        val deviceName = r.str()
        r.u8()
        val authRequired = r.u8()
        val clientPub = r.raw(65)
        val commit = r.raw(32)
        val secret = secrets[deviceId]
        val mode = if (secret != null) 2 else if (authRequired == 0) 0 else 1
        lastMode = mode
        val kp = Crypto.generateKeyPair()
        val helloH = ByteWriter().raw(Proto.MAGIC).u16(1).u16(0).raw(hostId).str("test-host").str(hostOs).u8(mode)
            .raw(Crypto.encodePublicKey(kp.public)).raw(Crypto.randomBytes(32)).bytes()
        channel.send(Proto.CH_CONTROL, Proto.HELLO_H, helloH)
        val f = channel.receive()
        if (f.type == Proto.AUTH_FAIL) {
            if (f.body[0].toInt() == Proto.FAIL_UNKNOWN_HOST) secrets.remove(deviceId)
            throw HandshakeException("client rejected", f.body[0].toInt())
        }
        val reveal = f.body
        if (!Crypto.sha256(reveal.copyOf(32)).contentEquals(commit)) {
            channel.send(Proto.CH_CONTROL, Proto.AUTH_FAIL, Bodies.u8(Proto.FAIL_BAD_COMMIT))
            error("bad commit")
        }
        val t = Crypto.transcript(helloC, helloH, reveal)
        if (mode == 1) {
            sasShown = Crypto.formatSas(Crypto.sas(t))
            if (!acceptSas) {
                channel.send(Proto.CH_CONTROL, Proto.AUTH_FAIL, Bodies.u8(Proto.FAIL_USER_REJECTED))
                throw HandshakeException("host user rejected", Proto.FAIL_USER_REJECTED)
            }
            channel.send(Proto.CH_CONTROL, Proto.SAS_OK)
            expect(channel, Proto.SAS_OK)
        }
        val keys = Crypto.keySchedule(Crypto.ecdh(kp.private, Crypto.decodePublicKey(clientPub)), secret ?: ByteArray(0), t)
        channel.send(Proto.CH_CONTROL, Proto.AUTH_H, Crypto.confirmMac(keys.kConfirm, Crypto.HOST_LABEL, t))
        val mac = expect(channel, Proto.AUTH_C)
        check(mac.contentEquals(Crypto.confirmMac(keys.kConfirm, Crypto.CLIENT_LABEL, t))) { "bad client mac" }
        if (mode != 0) channel.enableEncryption(sendKey = keys.kH2c, recvKey = keys.kC2h)
        if (mode == 1) secrets[deviceId] = expect(channel, Proto.PAIR)
        return Result(channel, mode, deviceName)
    }

    private fun expect(channel: FrameChannel, type: Int): ByteArray {
        val f = channel.receive()
        if (f.type == Proto.AUTH_FAIL) throw HandshakeException("peer sent AUTH_FAIL", f.body[0].toInt())
        check(f.channel == Proto.CH_CONTROL && f.type == type) { "expected 0x%02x got $f".format(type) }
        return f.body
    }

    companion object {
        /** A connected loopback pair: (client side, host side). The client side reports [link]. */
        fun socketPair(link: Int): Pair<Transport, Transport> {
            ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { server ->
                val hostSock = Socket(InetAddress.getLoopbackAddress(), server.localPort)
                val clientSock = server.accept()
                val client = object : Transport by SocketTransport(clientSock) {
                    override val link = link
                }
                return client to SocketTransport(hostSock)
            }
        }
    }
}

class MemoryPairStore : PairStore {
    val map = HashMap<String, ByteArray>()
    override fun secretFor(hostId: ByteArray) = map[hostId.toHex()]
    override fun save(hostId: ByteArray, hostName: String, hostOs: String, secret: ByteArray) { map[hostId.toHex()] = secret }
}

class AutoPrompt(private val accept: Boolean, private val delayMs: Long = 0) : SasPrompt {
    @Volatile var shown: String? = null
    @Volatile var dismissed = false
    override fun show(code: String, hostName: String, hostOs: String, decide: (Boolean) -> Unit) {
        shown = code
        Thread { Thread.sleep(delayMs); decide(accept) }.start()
    }
    override fun dismiss() { dismissed = true }
}
