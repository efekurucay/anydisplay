package dev.anydisplay.client.device

import android.content.Context
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.hardware.display.DisplayManager
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.os.Build
import android.util.DisplayMetrics
import android.view.Display
import android.view.InputDevice
import android.view.MotionEvent
import android.view.Surface
import dev.anydisplay.client.data.Settings
import dev.anydisplay.client.data.StreamOrientation
import dev.anydisplay.client.protocol.ConfigReq
import dev.anydisplay.client.protocol.DeviceInfo
import dev.anydisplay.client.protocol.Proto
import java.net.Inet4Address
import java.net.NetworkInterface
import kotlin.math.roundToInt

/** A hardware or software decoder for one codec, with its size limits. */
data class DecoderChoice(
    val codec: Int,
    val mime: String,
    val name: String,
    val hardware: Boolean,
    val lowLatency: Boolean,
    val maxW: Int,
    val maxH: Int,
    private val caps: MediaCodecInfo.VideoCapabilities?,
) {
    fun supports(w: Int, h: Int): Boolean = caps?.isSizeSupported(w, h) ?: (w <= maxW && h <= maxH)
}

/** Everything DEVICE_INFO and CONFIG_REQ need to know about this device. */
object DeviceCaps {
    fun mimeOf(codec: Int) = when (codec) {
        Proto.CODEC_HEVC -> "video/hevc"
        Proto.CODEC_AV1 -> "video/av01"
        else -> "video/avc"
    }

    private var decoderCache: Map<Int, DecoderChoice>? = null

    /** Best decoder per codec: hardware first, then one advertising low latency, then the largest size range. */
    @Synchronized
    fun decoders(): Map<Int, DecoderChoice> {
        decoderCache?.let { return it }
        val out = HashMap<Int, DecoderChoice>()
        val list = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
        for (codec in intArrayOf(Proto.CODEC_H264, Proto.CODEC_HEVC, Proto.CODEC_AV1)) {
            val mime = mimeOf(codec)
            val candidates = list.filter { info -> !info.isEncoder && info.supportedTypes.any { it.equals(mime, true) } }
                .mapNotNull { info ->
                    try {
                        val caps = info.getCapabilitiesForType(mime)
                        val v = caps.videoCapabilities
                        val hw = if (Build.VERSION.SDK_INT >= 29) info.isHardwareAccelerated else !isSoftwareName(info.name)
                        val ll = Build.VERSION.SDK_INT >= 30 &&
                            caps.isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency)
                        DecoderChoice(codec, mime, info.name, hw, ll, v?.supportedWidths?.upper ?: 1920, v?.supportedHeights?.upper ?: 1080, v)
                    } catch (_: Exception) {
                        null
                    }
                }
            candidates.maxWithOrNull(compareBy<DecoderChoice>({ it.hardware }, { it.lowLatency }, { it.maxW.toLong() * it.maxH }))
                ?.let { out[codec] = it }
        }
        decoderCache = out
        return out
    }

    private fun isSoftwareName(name: String) =
        name.startsWith("OMX.google.", true) || name.startsWith("c2.android.", true) || name.contains(".sw.", true)

    fun codecBits(): Int {
        var bits = 0
        val d = decoders()
        if (d.containsKey(Proto.CODEC_H264)) bits = bits or Proto.CODECS_BIT_H264
        if (d.containsKey(Proto.CODEC_HEVC)) bits = bits or Proto.CODECS_BIT_HEVC
        // AV1 software decoders cannot keep up with desktop streams; advertise AV1 only in hardware.
        if (d[Proto.CODEC_AV1]?.hardware == true) bits = bits or Proto.CODECS_BIT_AV1
        return bits
    }

    /** Codec to request: the setting, or for "auto" HEVC when a hardware HEVC decoder exists. */
    fun preferredCodec(settings: Settings): Int {
        val d = decoders()
        return when (settings.codec) {
            "hevc" -> if (d.containsKey(Proto.CODEC_HEVC)) Proto.CODEC_HEVC else Proto.CODEC_H264
            "h264" -> Proto.CODEC_H264
            else -> if (d[Proto.CODEC_HEVC]?.hardware == true) Proto.CODEC_HEVC else Proto.CODEC_H264
        }
    }

    private fun display(context: Context): Display =
        (context.getSystemService(Context.DISPLAY_SERVICE) as DisplayManager).getDisplay(Display.DEFAULT_DISPLAY)

    @Suppress("DEPRECATION")
    fun realMetrics(context: Context): DisplayMetrics = DisplayMetrics().also { display(context).getRealMetrics(it) }

    fun maxRefresh(context: Context): Float =
        display(context).supportedModes.maxOfOrNull { it.refreshRate } ?: display(context).refreshRate

    /** Screen size in the orientation the stream will use (current orientation for [StreamOrientation.AUTO]). */
    fun streamScreenSize(context: Context, orientation: StreamOrientation): Pair<Int, Int> {
        val m = realMetrics(context)
        val long = maxOf(m.widthPixels, m.heightPixels)
        val short = minOf(m.widthPixels, m.heightPixels)
        return when (orientation) {
            StreamOrientation.LANDSCAPE -> long to short
            StreamOrientation.PORTRAIT -> short to long
            StreamOrientation.AUTO ->
                if (context.resources.configuration.orientation == Configuration.ORIENTATION_PORTRAIT) short to long else long to short
        }
    }

    private fun penCaps(): Int {
        var caps = 0
        for (id in InputDevice.getDeviceIds()) {
            val dev = InputDevice.getDevice(id) ?: continue
            if (!dev.supportsSource(InputDevice.SOURCE_STYLUS)) continue
            caps = caps or Proto.PEN_CAP_HOVER or Proto.PEN_CAP_BARREL or Proto.PEN_CAP_ERASER
            if (dev.getMotionRange(MotionEvent.AXIS_PRESSURE) != null) caps = caps or Proto.PEN_CAP_PRESSURE
            if (dev.getMotionRange(MotionEvent.AXIS_TILT) != null) caps = caps or Proto.PEN_CAP_TILT
        }
        return caps
    }

    private fun maxTouches(context: Context): Int {
        val pm = context.packageManager
        return when {
            pm.hasSystemFeature(PackageManager.FEATURE_TOUCHSCREEN_MULTITOUCH_JAZZHAND) -> 10
            pm.hasSystemFeature(PackageManager.FEATURE_TOUCHSCREEN_MULTITOUCH_DISTINCT) -> 2
            else -> 1
        }
    }

    fun deviceInfo(context: Context, settings: Settings): DeviceInfo {
        val (w, h) = streamScreenSize(context, settings.orientation)
        val m = realMetrics(context)
        // xdpi/ydpi describe the panel's natural axes and do not rotate; swap them when the stream's x axis is the
        // panel's y axis.
        val rotation = display(context).rotation
        val currentLandscape = m.widthPixels > m.heightPixels
        val naturalLandscape = if (rotation == Surface.ROTATION_0 || rotation == Surface.ROTATION_180) currentLandscape else !currentLandscape
        val swapped = naturalLandscape != (w > h)
        val dec = decoders()[preferredCodec(settings)]
        return DeviceInfo(
            screenW = w, screenH = h,
            xdpi = if (swapped) m.ydpi else m.xdpi, ydpi = if (swapped) m.xdpi else m.ydpi,
            maxRefresh = maxRefresh(context),
            codecs = codecBits(),
            maxDecodeW = (dec?.maxW ?: 1920).coerceAtMost(0xFFFF), maxDecodeH = (dec?.maxH ?: 1080).coerceAtMost(0xFFFF),
            maxTouches = maxTouches(context),
            penCaps = penCaps(),
            audio = 1,
            model = "${Build.MANUFACTURER} ${Build.MODEL}",
            osVersion = "Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})",
        )
    }

    /** Refresh choices from settings that this display can show. */
    fun refreshChoices(context: Context): List<Int> {
        val max = maxRefresh(context).roundToInt()
        return listOf(30, 60, 90, 120).filter { it <= maxOf(max, 30) }
    }

    fun configRequest(context: Context, settings: Settings): ConfigReq {
        val codec = preferredCodec(settings)
        val (sw, sh) = streamScreenSize(context, settings.orientation)
        var w = (sw * settings.scale).roundToInt() and 1.inv()
        var h = (sh * settings.scale).roundToInt() and 1.inv()
        val dec = decoders()[codec]
        if (dec != null) {
            var guard = 0
            while (!dec.supports(w, h) && guard++ < 20) {
                w = (w * 0.9f).roundToInt() and 1.inv()
                h = (h * 0.9f).roundToInt() and 1.inv()
            }
        }
        val hz = settings.refreshHz.coerceAtMost(maxOf(maxRefresh(context).roundToInt(), 30))
        var flags = 0
        if (settings.hidpi) flags = flags or Proto.FLAG_HIDPI
        if (settings.audio) flags = flags or Proto.FLAG_AUDIO
        if (settings.clientCursor) flags = flags or Proto.FLAG_CLIENT_CURSOR
        if (settings.mirror) flags = flags or Proto.FLAG_MIRROR
        return ConfigReq(w, h, hz, codec, settings.bitrateKbps, settings.quality, flags)
    }

    private val CELLULAR = listOf("rmnet", "ccmni", "dummy", "p2p")

    /** IPv4 addresses a host on the LAN can reach, Wi-Fi first. */
    fun lanAddresses(): List<String> = try {
        NetworkInterface.getNetworkInterfaces().toList()
            .filter { it.isUp && !it.isLoopback && CELLULAR.none { p -> it.name.startsWith(p) } }
            .sortedByDescending { it.name.startsWith("wlan") }
            .flatMap { nif -> nif.inetAddresses.toList().filterIsInstance<Inet4Address>().map { it.hostAddress ?: "" } }
            .filter { it.isNotEmpty() }
    } catch (_: Exception) {
        emptyList()
    }
}
