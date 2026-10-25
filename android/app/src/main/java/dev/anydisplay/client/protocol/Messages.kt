package dev.anydisplay.client.protocol

/** Constants and message bodies from docs/PROTOCOL.md (protocol version 1). */
object Proto {
    const val PORT = 47820
    val MAGIC = byteArrayOf('A'.code.toByte(), 'N'.code.toByte(), 'Y'.code.toByte(), 'D'.code.toByte())
    const val MAJOR = 1
    const val MINOR = 0
    const val MAX_FRAME = 32 * 1024 * 1024

    const val CH_CONTROL = 0
    const val CH_VIDEO = 1
    const val CH_AUDIO = 2
    const val CH_CURSOR = 3
    const val CH_INPUT = 4

    const val HELLO_C = 0x01
    const val HELLO_H = 0x02
    const val AUTH_H = 0x03
    const val AUTH_C = 0x04
    const val AUTH_FAIL = 0x05
    const val SAS_OK = 0x06
    const val PAIR = 0x07
    const val REVEAL = 0x08

    const val DEVICE_INFO = 0x10
    const val CONFIG_REQ = 0x11
    const val CONFIG = 0x12
    const val PING = 0x13
    const val PONG = 0x14
    const val STATS = 0x15
    const val KEYFRAME_REQ = 0x16
    const val NOTICE = 0x17
    const val CLIPBOARD = 0x18
    const val BYE = 0x19

    const val VIDEO_FRAME = 0x20
    const val FRAME_ACK = 0x21

    const val CURSOR_POS = 0x30
    const val CURSOR_SHAPE = 0x31

    const val AUDIO_CONFIG = 0x40
    const val AUDIO_DATA = 0x41

    const val TOUCH = 0x50
    const val PEN = 0x51
    const val KEY = 0x52
    const val TEXT = 0x53
    const val SCROLL = 0x54
    const val MOUSE = 0x55

    const val LINK_LOOPBACK = 0
    const val LINK_NETWORK = 1
    const val LINK_USB_ACCESSORY = 2

    const val MODE_NONE = 0
    const val MODE_SAS = 1
    const val MODE_PAIRED = 2

    const val FAIL_BAD_VERSION = 1
    const val FAIL_AUTH_REQUIRED = 2
    const val FAIL_UNKNOWN_HOST = 3
    const val FAIL_BAD_MAC = 4
    const val FAIL_USER_REJECTED = 5
    const val FAIL_BUSY = 6
    const val FAIL_BAD_COMMIT = 7

    const val CODEC_H264 = 1
    const val CODEC_HEVC = 2
    const val CODEC_AV1 = 3

    const val CODECS_BIT_H264 = 1
    const val CODECS_BIT_HEVC = 2
    const val CODECS_BIT_AV1 = 4

    const val FLAG_HIDPI = 1
    const val FLAG_AUDIO = 2
    const val FLAG_CLIENT_CURSOR = 4
    const val FLAG_MIRROR = 8

    const val HOST_CAP_NATIVE_TOUCH = 1
    const val HOST_CAP_NATIVE_PEN = 2
    const val HOST_CAP_CLIPBOARD = 4
    const val HOST_CAP_AUDIO = 8

    const val PEN_CAP_PRESSURE = 1
    const val PEN_CAP_TILT = 2
    const val PEN_CAP_HOVER = 4
    const val PEN_CAP_ERASER = 8
    const val PEN_CAP_BARREL = 16

    const val BYE_USER = 0
    const val BYE_SHUTDOWN = 1
    const val BYE_ERROR = 2

    const val MIME_TEXT = "text/plain"

    fun failReasonName(reason: Int) = when (reason) {
        FAIL_BAD_VERSION -> "bad_version"
        FAIL_AUTH_REQUIRED -> "auth_required"
        FAIL_UNKNOWN_HOST -> "unknown_host"
        FAIL_BAD_MAC -> "bad_mac"
        FAIL_USER_REJECTED -> "user_rejected"
        FAIL_BUSY -> "busy"
        FAIL_BAD_COMMIT -> "bad_commit"
        else -> "reason_$reason"
    }

    fun codecName(codec: Int) = when (codec) {
        CODEC_H264 -> "H.264"
        CODEC_HEVC -> "HEVC"
        CODEC_AV1 -> "AV1"
        else -> "codec $codec"
    }
}

class Frame(val channel: Int, val type: Int, val body: ByteArray) {
    override fun toString() = "Frame(ch=$channel type=0x%02x len=${body.size})".format(type)
}

// ---------------------------------------------------------------- handshake bodies

class HelloC(
    val deviceId: ByteArray,
    val deviceName: String,
    val link: Int,
    val authRequired: Int,
    val clientPub: ByteArray,
    val clientCommit: ByteArray,
    val major: Int = Proto.MAJOR,
    val minor: Int = Proto.MINOR,
) {
    fun encode(): ByteArray = ByteWriter(160).raw(Proto.MAGIC).u16(major).u16(minor).raw(deviceId).str(deviceName)
        .u8(link).u8(authRequired).raw(clientPub).raw(clientCommit).bytes()
}

class HelloH(
    val magic: ByteArray,
    val major: Int,
    val minor: Int,
    val hostId: ByteArray,
    val hostName: String,
    val hostOs: String,
    val authMode: Int,
    val hostPub: ByteArray,
    val hostNonce: ByteArray,
) {
    companion object {
        /** Returns null when magic/major do not match so the caller can answer bad_version. */
        fun decode(body: ByteArray): HelloH? {
            val r = ByteReader(body)
            val magic = r.raw(4)
            val major = r.u16()
            if (!magic.contentEquals(Proto.MAGIC) || major != Proto.MAJOR) return null
            return HelloH(magic, major, r.u16(), r.raw(16), r.str(), r.str(), r.u8(), r.raw(65), r.raw(32))
        }
    }
}

// ---------------------------------------------------------------- session bodies

data class DeviceInfo(
    val screenW: Int,
    val screenH: Int,
    val xdpi: Float,
    val ydpi: Float,
    val maxRefresh: Float,
    val codecs: Int,
    val maxDecodeW: Int,
    val maxDecodeH: Int,
    val maxTouches: Int,
    val penCaps: Int,
    val audio: Int,
    val model: String,
    val osVersion: String,
) {
    fun encode(): ByteArray = ByteWriter(96).u16(screenW).u16(screenH).f32(xdpi).f32(ydpi).f32(maxRefresh)
        .u32(codecs).u16(maxDecodeW).u16(maxDecodeH).u8(maxTouches).u32(penCaps).u8(audio)
        .str(model).str(osVersion).bytes()
}

data class ConfigReq(
    val width: Int,
    val height: Int,
    val refreshHz: Int,
    val codec: Int,
    val bitrateKbps: Int,
    val quality: Int,
    val flags: Int,
) {
    fun encode(): ByteArray = ByteWriter(16).u16(width).u16(height).u16(refreshHz).u8(codec).u32(bitrateKbps)
        .u8(quality).u8(flags).bytes()
}

data class StreamConfig(
    val streamId: Long,
    val width: Int,
    val height: Int,
    val refreshHz: Int,
    val codec: Int,
    val bitrateKbps: Long,
    val flags: Int,
    val logicalW: Int,
    val logicalH: Int,
    val hostCaps: Long,
) {
    val audio get() = flags and Proto.FLAG_AUDIO != 0
    val clientCursor get() = flags and Proto.FLAG_CLIENT_CURSOR != 0

    companion object {
        fun decode(body: ByteArray): StreamConfig {
            val r = ByteReader(body)
            return StreamConfig(r.u32(), r.u16(), r.u16(), r.u16(), r.u8(), r.u32(), r.u8(), r.u16(), r.u16(), r.u32())
        }
    }
}

data class Stats(
    val framesDecoded: Int,
    val framesDropped: Int,
    val avgDecodeUs: Int,
    val rttUs: Int,
    val bytesReceived: Long,
) {
    fun encode(): ByteArray = ByteWriter(20).u32(framesDecoded).u32(framesDropped).u32(avgDecodeUs).u32(rttUs)
        .u32(bytesReceived.coerceAtMost(0xFFFFFFFFL)).bytes()
}

class VideoFrame(
    val streamId: Long,
    val frameId: Long,
    val ptsUs: Long,
    val flags: Int,
    /** Annex-B access unit, located at [dataOffset] in [buffer]. */
    val buffer: ByteArray,
    val dataOffset: Int,
    val dataLength: Int,
) {
    val keyframe get() = flags and 1 != 0

    companion object {
        const val HEADER = 4 + 4 + 8 + 1

        fun decode(body: ByteArray): VideoFrame {
            val r = ByteReader(body)
            return VideoFrame(r.u32(), r.u32(), r.u64(), r.u8(), body, HEADER, body.size - HEADER)
        }
    }
}

object Bodies {
    fun ping(seq: Long, timeUs: Long) = ByteWriter(12).u32(seq).u64(timeUs).bytes()
    fun frameAck(streamId: Long, frameId: Long, decodeUs: Long) =
        ByteWriter(12).u32(streamId).u32(frameId).u32(decodeUs.coerceIn(0, 0xFFFFFFFFL)).bytes()
    fun clipboard(mime: String, data: ByteArray) = ByteWriter(8 + data.size).str(mime).blob(data).bytes()
    fun u8(v: Int) = byteArrayOf(v.toByte())

    fun key(down: Boolean, usage: Int, modifiers: Int) =
        ByteWriter(5).u8(if (down) 1 else 0).u16(usage).u16(modifiers).bytes()

    fun text(s: String) = ByteWriter(2 + s.length * 3).str(s).bytes()

    fun mouse(action: Int, button: Int, x: Int, y: Int) = ByteWriter(6).u8(action).u8(button).u16(x).u16(y).bytes()

    fun scroll(x: Int, y: Int, dx: Int, dy: Int, phase: Int) =
        ByteWriter(9).u16(x).u16(y).i16(dx.coerceIn(-32768, 32767)).i16(dy.coerceIn(-32768, 32767)).u8(phase).bytes()
}

data class TouchContact(val id: Int, val action: Int, val x: Int, val y: Int, val pressure: Int, val size: Int)

object TouchAction {
    const val DOWN = 0
    const val MOVE = 1
    const val UP = 2
    const val CANCEL = 3
}

fun encodeTouch(contacts: List<TouchContact>): ByteArray {
    val w = ByteWriter(1 + contacts.size * 10).u8(contacts.size)
    for (c in contacts) w.u8(c.id).u8(c.action).u16(c.x).u16(c.y).u16(c.pressure).u16(c.size)
    return w.bytes()
}

data class PenSample(
    val action: Int,
    val tool: Int,
    val buttons: Int,
    val x: Int,
    val y: Int,
    val pressure: Int,
    val tiltX: Int,
    val tiltY: Int,
    val twist: Int,
    val timeMs: Long,
)

object PenAction {
    const val DOWN = 0
    const val MOVE = 1
    const val UP = 2
    const val HOVER_ENTER = 3
    const val HOVER_MOVE = 4
    const val HOVER_EXIT = 5
    const val CANCEL = 6
}

/** Encodes one PEN message per 255 samples (the count field is a u8). */
fun encodePen(samples: List<PenSample>): List<ByteArray> = samples.chunked(255).map { chunk ->
    val w = ByteWriter(1 + chunk.size * 20).u8(chunk.size)
    for (s in chunk) {
        w.u8(s.action).u8(s.tool).u8(s.buttons).u16(s.x).u16(s.y).u16(s.pressure)
            .i16(s.tiltX).i16(s.tiltY).u16(s.twist).u32(s.timeMs and 0xFFFFFFFFL)
    }
    w.bytes()
}

object MouseAction {
    const val MOVE = 0
    const val DOWN = 1
    const val UP = 2
}

object MouseButton {
    const val LEFT = 0
    const val RIGHT = 1
    const val MIDDLE = 2
}

object ScrollPhase {
    const val BEGIN = 0
    const val CHANGE = 1
    const val END = 2
    const val MOMENTUM = 3
}

object KeyMods {
    const val CTRL = 1
    const val SHIFT = 2
    const val ALT = 4
    const val META = 8
}

/** Normalizes a coordinate inside [origin, origin+extent] to the protocol's u16 range. */
fun normalize(v: Float, origin: Float, extent: Float): Int {
    if (extent <= 0f) return 0
    val n = (v - origin) / extent
    return (n.coerceIn(0f, 1f) * 65535f + 0.5f).toInt()
}
