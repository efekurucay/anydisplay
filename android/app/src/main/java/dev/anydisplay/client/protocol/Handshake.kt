package dev.anydisplay.client.protocol

import java.security.KeyPair
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/** Pairing secrets the client stores per host (PROTOCOL.md 4.2 step 11). */
interface PairStore {
    fun secretFor(hostId: ByteArray): ByteArray?
    fun save(hostId: ByteArray, hostName: String, hostOs: String, secret: ByteArray)
    fun touch(hostId: ByteArray) {}
}

/** Asks the user to compare the 6-digit code. [decide] must be called exactly once, from any thread. */
interface SasPrompt {
    fun show(code: String, hostName: String, hostOs: String, decide: (Boolean) -> Unit)
    fun dismiss()
}

class HandshakeException(message: String, val reason: Int = 0, val sentFail: Boolean = false) : Exception(message)

class HandshakeResult(
    val mode: Int,
    val hostId: ByteArray,
    val hostName: String,
    val hostOs: String,
    val keys: SessionKeys,
    val transcript: ByteArray,
)

/**
 * Client side of the handshake in PROTOCOL.md section 4.2. Runs on the connection thread and blocks.
 * [keyPair], [clientNonce] and [pairSecret] are injectable so tests can reproduce tools/vectors.json.
 */
class ClientHandshake(
    private val channel: FrameChannel,
    private val deviceId: ByteArray,
    private val deviceName: String,
    private val link: Int,
    private val pairs: PairStore,
    private val prompt: SasPrompt,
    private val setTimeout: (Long, String) -> Unit = { _, _ -> },
    private val keyPair: KeyPair = Crypto.generateKeyPair(),
    private val clientNonce: ByteArray = Crypto.randomBytes(32),
    private val pairSecret: () -> ByteArray = { Crypto.randomBytes(32) },
    private val log: (String) -> Unit = {},
) {
    companion object {
        const val STEP_TIMEOUT_MS = 10_000L
        const val USER_TIMEOUT_MS = 120_000L
    }

    private fun fail(reason: Int, message: String): Nothing {
        try { channel.send(Proto.CH_CONTROL, Proto.AUTH_FAIL, Bodies.u8(reason)) } catch (_: Exception) {}
        throw HandshakeException(message, reason, sentFail = true)
    }

    private fun expect(type: Int, timeoutMs: Long = STEP_TIMEOUT_MS): ByteArray {
        setTimeout(timeoutMs, "waiting for type 0x%02x".format(type))
        val f = channel.receive()
        if (f.channel == Proto.CH_CONTROL && f.type == Proto.AUTH_FAIL) {
            val reason = if (f.body.isNotEmpty()) f.body[0].toInt() and 0xFF else 0
            throw HandshakeException("host sent AUTH_FAIL(${Proto.failReasonName(reason)})", reason)
        }
        if (f.channel != Proto.CH_CONTROL || f.type != type) {
            throw HandshakeException("expected type 0x%02x, got ch=%d type=0x%02x".format(type, f.channel, f.type))
        }
        return f.body
    }

    fun run(): HandshakeResult {
        val authRequired = if (link == Proto.LINK_NETWORK) 1 else 0
        val clientPub = Crypto.encodePublicKey(keyPair.public)
        val helloC = HelloC(deviceId, deviceName, link, authRequired, clientPub, Crypto.sha256(clientNonce)).encode()
        channel.send(Proto.CH_CONTROL, Proto.HELLO_C, helloC)

        val helloHBody = expect(Proto.HELLO_H)
        val helloH = try {
            HelloH.decode(helloHBody)
        } catch (e: ProtocolException) {
            fail(Proto.FAIL_BAD_VERSION, "malformed HELLO_H: ${e.message}")
        } ?: fail(Proto.FAIL_BAD_VERSION, "host speaks another protocol version")
        val mode = helloH.authMode
        log("HELLO_H host='${helloH.hostName}' os='${helloH.hostOs}' mode=$mode")
        if (mode !in Proto.MODE_NONE..Proto.MODE_PAIRED) fail(Proto.FAIL_BAD_VERSION, "unknown auth mode $mode")
        if (mode == Proto.MODE_NONE && authRequired == 1) fail(Proto.FAIL_AUTH_REQUIRED, "host offered no auth on a network link")
        val psk = if (mode == Proto.MODE_PAIRED) {
            pairs.secretFor(helloH.hostId) ?: fail(Proto.FAIL_UNKNOWN_HOST, "host claims pairing we do not have")
        } else ByteArray(0)
        val hostPub = try {
            Crypto.decodePublicKey(helloH.hostPub)
        } catch (e: Exception) {
            fail(Proto.FAIL_BAD_MAC, "invalid host public key: ${e.message}")
        }

        channel.send(Proto.CH_CONTROL, Proto.REVEAL, clientNonce)
        val transcript = Crypto.transcript(helloC, helloHBody, clientNonce)

        if (mode == Proto.MODE_SAS) confirmSas(transcript, helloH)

        val keys = Crypto.keySchedule(Crypto.ecdh(keyPair.private, hostPub), psk, transcript)
        val authH = expect(Proto.AUTH_H)
        if (!Crypto.constantTimeEquals(authH, Crypto.confirmMac(keys.kConfirm, Crypto.HOST_LABEL, transcript))) {
            fail(Proto.FAIL_BAD_MAC, "host confirmation MAC mismatch")
        }
        channel.send(Proto.CH_CONTROL, Proto.AUTH_C, Crypto.confirmMac(keys.kConfirm, Crypto.CLIENT_LABEL, transcript))
        if (mode != Proto.MODE_NONE) channel.enableEncryption(sendKey = keys.kC2h, recvKey = keys.kH2c)
        if (mode == Proto.MODE_SAS) {
            val secret = pairSecret()
            channel.send(Proto.CH_CONTROL, Proto.PAIR, secret)
            pairs.save(helloH.hostId, helloH.hostName, helloH.hostOs, secret)
        } else if (mode == Proto.MODE_PAIRED) {
            pairs.touch(helloH.hostId)
        }
        return HandshakeResult(mode, helloH.hostId, helloH.hostName, helloH.hostOs, keys, transcript)
    }

    /**
     * Step 6: show the code, send SAS_OK or AUTH_FAIL(user_rejected) when the user decides, and proceed only
     * after both sending and receiving SAS_OK. The host's answer may arrive before or after the user's.
     */
    private fun confirmSas(transcript: ByteArray, helloH: HelloH) {
        val code = Crypto.formatSas(Crypto.sas(transcript))
        log("pairing code $code")
        val decided = CountDownLatch(1)
        val accepted = AtomicReference<Boolean?>(null)
        prompt.show(code, helloH.hostName, helloH.hostOs) { ok ->
            if (accepted.compareAndSet(null, ok)) {
                // The UI calls this on its main thread, where blocking socket writes are not allowed, and the
                // connection thread may be blocked reading the host's answer, so write from a short-lived thread.
                Thread({
                    try {
                        if (ok) channel.send(Proto.CH_CONTROL, Proto.SAS_OK)
                        else channel.send(Proto.CH_CONTROL, Proto.AUTH_FAIL, Bodies.u8(Proto.FAIL_USER_REJECTED))
                    } catch (e: Exception) {
                        log("sending the pairing answer failed: ${e.message}")
                    }
                    if (!ok) channel.close()
                    decided.countDown()
                }, "anydisplay-sas-answer").start()
            }
        }
        try {
            try {
                expect(Proto.SAS_OK, USER_TIMEOUT_MS)
            } catch (e: Exception) {
                if (accepted.get() == false) throw HandshakeException("pairing rejected on this device", Proto.FAIL_USER_REJECTED, true)
                throw e
            }
            setTimeout(USER_TIMEOUT_MS, "waiting for the user to confirm the code")
            if (!decided.await(USER_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
                fail(Proto.FAIL_USER_REJECTED, "pairing code not confirmed in time")
            }
            if (accepted.get() != true) throw HandshakeException("pairing rejected on this device", Proto.FAIL_USER_REJECTED, true)
        } finally {
            if (accepted.compareAndSet(null, false)) decided.countDown()
            prompt.dismiss()
        }
    }
}
