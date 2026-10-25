package dev.anydisplay.client.audio

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import dev.anydisplay.client.Elog

/**
 * Plays AUDIO_DATA (PCM s16le interleaved) through a low-latency AudioTrack.
 * The track's own buffer (about [BUFFER_MS]) is the jitter buffer: writes never block, and whatever does not fit
 * because playback is too far behind is dropped, so latency cannot build up.
 */
class AudioPlayer {
    private var track: AudioTrack? = null
    private var frameBytes = 4
    private var sampleRate = 48000
    private var framesWritten = 0L
    var droppedBytes = 0L
        private set

    @Synchronized
    fun configure(rate: Int, channelCount: Int, codec: Int) {
        release()
        if (codec != 0 || rate <= 0 || channelCount !in 1..2) {
            Elog.w("unsupported audio config rate=$rate ch=$channelCount codec=$codec")
            return
        }
        sampleRate = rate
        frameBytes = channelCount * 2
        val mask = if (channelCount == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO
        val min = AudioTrack.getMinBufferSize(rate, mask, AudioFormat.ENCODING_PCM_16BIT)
        // AudioFlinger starts a streaming track only once its buffer is full, so the buffer must stay small.
        val size = maxOf(min, rate * frameBytes * BUFFER_MS / 1000)
        track = try {
            AudioTrack.Builder()
                .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC).build())
                .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(rate).setChannelMask(mask).build())
                .setBufferSizeInBytes(size)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
                .build()
                .also { it.play() }
        } catch (e: Exception) {
            Elog.e("AudioTrack creation failed", e)
            null
        }
        framesWritten = 0
        droppedBytes = 0
        Elog.i("audio: ${rate}Hz x$channelCount buffer=${track?.bufferSizeInFrames} frames")
    }

    @Synchronized
    fun write(data: ByteArray, offset: Int, length: Int) {
        val t = track ?: return
        val n = length - length % frameBytes
        val written = t.write(data, offset, n, AudioTrack.WRITE_NON_BLOCKING)
        if (written > 0) framesWritten += written / frameBytes
        if (written < n) droppedBytes += n - written.coerceAtLeast(0)
    }

    @Synchronized
    fun release() {
        val t = track ?: return
        track = null
        Elog.i("audio: stopped after ${framesWritten * 1000 / sampleRate} ms played, ${droppedBytes * 1000 / (sampleRate * frameBytes)} ms dropped")
        try { t.stop() } catch (_: Exception) {}
        t.release()
    }

    companion object {
        const val BUFFER_MS = 60
    }
}
