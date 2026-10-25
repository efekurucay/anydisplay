package dev.anydisplay.client.protocol

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class HandshakeTest {
    private val pool = Executors.newCachedThreadPool()
    private val deviceId = Crypto.randomBytes(16)

    private class Outcome(val client: Result<HandshakeResult>, val host: Result<TestHost.Result>, val clientChannel: FrameChannel)

    private fun run(link: Int, host: TestHost, pairs: PairStore, prompt: SasPrompt): Outcome {
        val (ct, ht) = TestHost.socketPair(link)
        val cc = FrameChannel(ct)
        val hc = FrameChannel(ht)
        val hostF = pool.submit(Callable { runCatching { host.handshake(hc) }.also { if (it.isFailure) hc.close() } })
        val clientR = runCatching {
            ClientHandshake(cc, deviceId, "Test Phone", link, pairs, prompt).run()
        }
        if (clientR.isFailure) cc.close()
        return Outcome(clientR, hostF.get(10, TimeUnit.SECONDS), cc)
    }

    @Test
    fun loopbackIsPlain() {
        val o = run(Proto.LINK_LOOPBACK, TestHost(), MemoryPairStore(), AutoPrompt(true))
        assertEquals(0, o.client.getOrThrow().mode)
        assertEquals(0, o.host.getOrThrow().mode)
        assertFalse(o.clientChannel.encrypted)
    }

    @Test
    fun networkPairsThenReconnectsWithSecret() {
        val host = TestHost()
        val pairs = MemoryPairStore()
        val prompt = AutoPrompt(true, delayMs = 50)
        val first = run(Proto.LINK_NETWORK, host, pairs, prompt)
        val r = first.client.getOrThrow()
        assertEquals(1, r.mode)
        assertEquals(host.sasShown, prompt.shown)
        assertTrue(prompt.dismissed)
        assertTrue(first.clientChannel.encrypted)
        // PAIR reached the host and matches what the client stored.
        assertArrayEquals(pairs.map[host.hostId.toHex()], host.secrets[deviceId.toHex()])

        // Encrypted traffic both ways after the handshake.
        val hostCh = first.host.getOrThrow().channel
        first.clientChannel.send(Proto.CH_INPUT, Proto.TEXT, Bodies.text("héllo"))
        val got = hostCh.receive()
        assertEquals(Proto.TEXT, got.type)
        assertEquals("héllo", ByteReader(got.body).str())
        hostCh.send(Proto.CH_CONTROL, Proto.KEYFRAME_REQ)
        assertEquals(0, first.clientChannel.receive().body.size)

        val second = run(Proto.LINK_NETWORK, host, pairs, AutoPrompt(true))
        assertEquals(2, second.client.getOrThrow().mode)
        assertTrue(second.clientChannel.encrypted)
    }

    @Test
    fun hostWithStaleSecretGetsUnknownHost() {
        val host = TestHost()
        host.secrets[deviceId.toHex()] = Crypto.randomBytes(32)
        val o = run(Proto.LINK_NETWORK, host, MemoryPairStore(), AutoPrompt(true))
        assertEquals(Proto.FAIL_UNKNOWN_HOST, (o.client.exceptionOrNull() as HandshakeException).reason)
        assertTrue(host.secrets.isEmpty())
        // The host retries with mode 1 and succeeds.
        assertEquals(1, run(Proto.LINK_NETWORK, host, MemoryPairStore(), AutoPrompt(true)).client.getOrThrow().mode)
    }

    @Test
    fun userRejectsOnPhone() {
        val o = run(Proto.LINK_NETWORK, TestHost(), MemoryPairStore(), AutoPrompt(false))
        assertEquals(Proto.FAIL_USER_REJECTED, (o.client.exceptionOrNull() as HandshakeException).reason)
        assertEquals(Proto.FAIL_USER_REJECTED, (o.host.exceptionOrNull() as HandshakeException).reason)
    }

    @Test
    fun hostRejectsWhileUserDecides() {
        val prompt = AutoPrompt(true, delayMs = 2000)
        val o = run(Proto.LINK_NETWORK, TestHost(acceptSas = false), MemoryPairStore(), prompt)
        assertEquals(Proto.FAIL_USER_REJECTED, (o.client.exceptionOrNull() as HandshakeException).reason)
        assertTrue(prompt.dismissed)
    }

    @Test
    fun hostMustNotSkipAuthOnNetwork() {
        // A host that answers mode 0 to auth_required=1 is refused.
        val (ct, ht) = TestHost.socketPair(Proto.LINK_NETWORK)
        val cc = FrameChannel(ct)
        val hc = FrameChannel(ht)
        val f = pool.submit(Callable {
            val helloC = hc.receive()
            val kp = Crypto.generateKeyPair()
            hc.send(Proto.CH_CONTROL, Proto.HELLO_H, ByteWriter().raw(Proto.MAGIC).u16(1).u16(0).raw(ByteArray(16)).str("x")
                .str("y").u8(0).raw(Crypto.encodePublicKey(kp.public)).raw(ByteArray(32)).bytes())
            assertNotNull(helloC)
            hc.receive()
        })
        try {
            ClientHandshake(cc, deviceId, "p", Proto.LINK_NETWORK, MemoryPairStore(), AutoPrompt(true)).run()
            fail("expected failure")
        } catch (e: HandshakeException) {
            assertEquals(Proto.FAIL_AUTH_REQUIRED, e.reason)
        }
        val fail = f.get(5, TimeUnit.SECONDS)
        assertEquals(Proto.AUTH_FAIL, fail.type)
        assertEquals(Proto.FAIL_AUTH_REQUIRED, fail.body[0].toInt())
    }

    /** Drives the client state machine with the exact bytes of tools/vectors.json. */
    @Test
    fun vectorHandshakeByteForByte() {
        val v = VectorsTest.v
        val m1 = v.getJSONObject("mode1")
        val pairSecret = ByteArray(32) { 0x42 }
        val (ct, ht) = TestHost.socketPair(Proto.LINK_NETWORK)
        val cc = FrameChannel(ct)
        val hc = FrameChannel(ht)
        val prompt = AutoPrompt(true)
        val pairs = MemoryPairStore()
        val hostSide = pool.submit(Callable {
            val helloC = hc.receive()
            assertArrayEquals(VectorsTest.hex("hello_c_body"), helloC.body)
            hc.send(Proto.CH_CONTROL, Proto.HELLO_H, VectorsTest.hex("hello_h_body"))
            assertArrayEquals(VectorsTest.hex("reveal_body"), hc.receive().body)
            hc.send(Proto.CH_CONTROL, Proto.SAS_OK)
            assertEquals(Proto.SAS_OK, hc.receive().type)
            hc.send(Proto.CH_CONTROL, Proto.AUTH_H, VectorsTest.hex("auth_h", m1))
            val authC = hc.receive()
            assertEquals(Proto.AUTH_C, authC.type)
            assertArrayEquals(VectorsTest.hex("auth_c", m1), authC.body)
            // The client's first encrypted frame is PAIR under k_c2h, seq 0.
            val pair = hc.receive()
            assertEquals(Proto.PAIR, pair.type)
            assertArrayEquals(pairSecret, FrameCipher(VectorsTest.hex("k_c2h", m1)).decrypt(0, Proto.PAIR, pair.body))
            // Raw vector ciphertext of PING(7, 123456789) must decrypt on the client.
            hc.send(Proto.CH_CONTROL, Proto.PING, VectorsTest.hex("encrypted_ping_h2c_seq0", m1))
        })
        val r = ClientHandshake(
            cc, VectorsTest.deviceId, "Phone", Proto.LINK_NETWORK, pairs, prompt,
            keyPair = VectorsTest.clientKeys,
            clientNonce = VectorsTest.hex("client_nonce"),
            pairSecret = { pairSecret },
        ).run()
        hostSide.get(10, TimeUnit.SECONDS)
        assertEquals(1, r.mode)
        assertEquals(v.getString("sas"), prompt.shown)
        assertArrayEquals(VectorsTest.hex("transcript"), r.transcript)
        assertArrayEquals(pairSecret, pairs.map[VectorsTest.hostId.toHex()])
        val ping = cc.receive()
        assertEquals(Proto.PING, ping.type)
        assertArrayEquals(VectorsTest.hex("ping_plaintext_body"), ping.body)
    }
}
