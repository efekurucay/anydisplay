package dev.anydisplay.client.protocol

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.io.OutputStream

class FramingTest {
    private class MemTransport(bytes: ByteArray = ByteArray(0)) : Transport {
        val out = ByteArrayOutputStream()
        override val input: InputStream = ByteArrayInputStream(bytes)
        override val output: OutputStream = out
        override val link = 0
        override val peer = "mem"
        override fun close() {}
    }

    @Test
    fun frameLayout() {
        val t = MemTransport()
        FrameChannel(t).send(Proto.CH_VIDEO, Proto.FRAME_ACK, Bodies.frameAck(1, 2, 3))
        assertEquals("0e00000001210100000002000000" + "03000000", t.out.toByteArray().toHex())
    }

    @Test
    fun readsBackAndHandlesEmptyBodies() {
        val w = MemTransport()
        val ch = FrameChannel(w)
        ch.send(Proto.CH_CONTROL, Proto.KEYFRAME_REQ)
        ch.send(Proto.CH_INPUT, Proto.KEY, Bodies.key(true, 0x04, KeyMods.SHIFT))
        val r = FrameChannel(MemTransport(w.out.toByteArray()))
        val a = r.receive()
        assertEquals(Proto.KEYFRAME_REQ, a.type)
        assertEquals(0, a.body.size)
        val b = r.receive()
        assertEquals(Proto.CH_INPUT, b.channel)
        assertArrayEquals(byteArrayOf(1, 4, 0, 2, 0), b.body)
    }

    @Test(expected = ProtocolException::class)
    fun rejectsOversizedFrame() {
        val bad = ByteWriter().u32(Proto.MAX_FRAME + 1L).u8(0).u8(0).bytes()
        FrameChannel(MemTransport(bad)).receive()
    }

    @Test(expected = ProtocolException::class)
    fun rejectsTinyFrame() {
        FrameChannel(MemTransport(ByteWriter().u32(1).u8(0).u8(0).bytes())).receive()
    }

    @Test
    fun encryptedRoundTripWithSequence() {
        val k1 = Crypto.randomBytes(32)
        val k2 = Crypto.randomBytes(32)
        val w = MemTransport()
        val sender = FrameChannel(w).apply { enableEncryption(k1, k2) }
        repeat(3) { sender.send(Proto.CH_CONTROL, Proto.PING, Bodies.ping(it.toLong(), 99)) }
        val bytes = w.out.toByteArray()
        // length covers channel+type+ciphertext+tag: 2 + 12 + 16
        assertEquals(30, bytes[0].toInt())
        val receiver = FrameChannel(MemTransport(bytes)).apply { enableEncryption(k2, k1) }
        repeat(3) { assertEquals(it.toLong(), ByteReader(receiver.receive().body).u32()) }
    }

    @Test
    fun configAndDeviceInfoLayouts() {
        val req = ConfigReq(1920, 1080, 60, Proto.CODEC_HEVC, 0, 80, Proto.FLAG_AUDIO).encode()
        assertEquals("80073804" + "3c00" + "02" + "00000000" + "50" + "02", req.toHex())
        val cfg = StreamConfig.decode(
            ByteWriter().u32(7).u16(2340).u16(1080).u16(60).u8(1).u32(8000).u8(4).u16(1170).u16(540).u32(5).bytes()
        )
        assertEquals(StreamConfig(7, 2340, 1080, 60, 1, 8000, 4, 1170, 540, 5), cfg)
        val info = DeviceInfo(2340, 1080, 400f, 401f, 60f, 3, 4096, 2304, 10, 31, 1, "M", "10").encode()
        val r = ByteReader(info)
        assertEquals(2340, r.u16()); assertEquals(1080, r.u16())
        assertEquals(400f, r.f32()); assertEquals(401f, r.f32()); assertEquals(60f, r.f32())
        assertEquals(3L, r.u32()); assertEquals(4096, r.u16()); assertEquals(2304, r.u16())
        assertEquals(10, r.u8()); assertEquals(31L, r.u32()); assertEquals(1, r.u8())
        assertEquals("M", r.str()); assertEquals("10", r.str()); assertEquals(0, r.remaining)
    }

    @Test
    fun penAndTouchLayouts() {
        val pen = encodePen(listOf(PenSample(PenAction.MOVE, 1, 3, 65535, 0, 1000, -4500, 9000, 0, 0x1_0000_0005L)))
        assertEquals(1, pen.size)
        assertEquals("01" + "01" + "01" + "03" + "ffff" + "0000" + "e803" + "6cee" + "2823" + "0000" + "05000000", pen[0].toHex())
        assertEquals(2, encodePen(List(300) { PenSample(1, 0, 0, 0, 0, 0, 0, 0, 0, 0) }).size)
        val touch = encodeTouch(listOf(TouchContact(3, TouchAction.UP, 1, 2, 3, 4)))
        assertEquals("01" + "03" + "02" + "0100" + "0200" + "0300" + "0400", touch.toHex())
        assertEquals("0000ffff" + "0100" + "ffff" + "03", Bodies.scroll(0, 65535, 1, -1, 3).toHex())
    }

    @Test
    fun normalizeClamps() {
        assertEquals(0, normalize(-5f, 0f, 100f))
        assertEquals(65535, normalize(150f, 0f, 100f))
        assertEquals(32768, normalize(60f, 10f, 100f))
    }

    @Test
    fun prioritySenderDrainsControlBeforeVideo() {
        val w = MemTransport()
        val ch = FrameChannel(w)
        val s = PrioritySender(ch) { throw it }
        // Queue before starting so the drain order is observable.
        s.send(Proto.CH_VIDEO, Proto.FRAME_ACK, Bodies.frameAck(1, 1, 1))
        s.send(Proto.CH_AUDIO, 0x99)
        s.send(Proto.CH_INPUT, Proto.TOUCH, encodeTouch(emptyList()))
        s.send(Proto.CH_CONTROL, Proto.PING, Bodies.ping(1, 1))
        s.send(Proto.CH_CURSOR, 0x98)
        s.start()
        s.drain(2000)
        s.stop()
        val r = FrameChannel(MemTransport(w.out.toByteArray()))
        assertEquals(listOf(0, 4, 3, 2, 1), List(5) { r.receive().channel })
    }
}
