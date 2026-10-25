package dev.anydisplay.client.protocol

import java.math.BigInteger
import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.MessageDigest
import java.security.PrivateKey
import java.security.PublicKey
import java.security.SecureRandom
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPrivateKeySpec
import java.security.spec.ECPublicKeySpec
import javax.crypto.Cipher
import javax.crypto.KeyAgreement
import javax.crypto.Mac
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/** Crypto primitives of PROTOCOL.md section 4, built only on java.security / javax.crypto. */
object Crypto {
    private val random = SecureRandom()

    fun randomBytes(n: Int): ByteArray = ByteArray(n).also { random.nextBytes(it) }

    fun sha256(vararg parts: ByteArray): ByteArray {
        val md = MessageDigest.getInstance("SHA-256")
        for (p in parts) md.update(p)
        return md.digest()
    }

    fun hmacSha256(key: ByteArray, vararg parts: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        // An empty HMAC key is legal, but SecretKeySpec rejects it; HMAC pads keys to the block size with zeros.
        mac.init(SecretKeySpec(if (key.isEmpty()) ByteArray(32) else key, "HmacSHA256"))
        for (p in parts) mac.update(p)
        return mac.doFinal()
    }

    /** RFC 5869 HKDF with SHA-256. */
    fun hkdfSha256(salt: ByteArray, ikm: ByteArray, info: ByteArray, length: Int): ByteArray {
        require(length <= 255 * 32)
        val prk = hmacSha256(salt, ikm)
        val out = ByteArray(length)
        var t = ByteArray(0)
        var pos = 0
        var counter = 1
        while (pos < length) {
            t = hmacSha256(prk, t, info, byteArrayOf(counter.toByte()))
            val n = minOf(t.size, length - pos)
            System.arraycopy(t, 0, out, pos, n)
            pos += n
            counter++
        }
        return out
    }

    fun constantTimeEquals(a: ByteArray, b: ByteArray) = MessageDigest.isEqual(a, b)

    // ------------------------------------------------------------ P-256

    private val p256: ECParameterSpec by lazy {
        val kpg = KeyPairGenerator.getInstance("EC")
        kpg.initialize(ECGenParameterSpec("secp256r1"))
        (kpg.generateKeyPair().public as ECPublicKey).params
    }

    fun generateKeyPair(): KeyPair {
        val kpg = KeyPairGenerator.getInstance("EC")
        kpg.initialize(ECGenParameterSpec("secp256r1"), random)
        return kpg.generateKeyPair()
    }

    /** Builds a key pair from known values; used to reproduce test vectors. */
    fun keyPairFromRaw(privateScalar: ByteArray, publicX962: ByteArray): KeyPair {
        val priv = KeyFactory.getInstance("EC").generatePrivate(ECPrivateKeySpec(BigInteger(1, privateScalar), p256))
        return KeyPair(decodePublicKey(publicX962), priv)
    }

    /** Uncompressed X9.62 encoding: 0x04 || X || Y, 65 bytes. */
    fun encodePublicKey(key: PublicKey): ByteArray {
        val w = (key as ECPublicKey).w
        return byteArrayOf(4) + fixed32(w.affineX) + fixed32(w.affineY)
    }

    fun decodePublicKey(x962: ByteArray): PublicKey {
        if (x962.size != 65 || x962[0].toInt() != 4) throw ProtocolException("bad public key encoding")
        val x = BigInteger(1, x962.copyOfRange(1, 33))
        val y = BigInteger(1, x962.copyOfRange(33, 65))
        val curve = p256.curve
        val p = (curve.field as java.security.spec.ECFieldFp).p
        if (x >= p || y >= p) throw ProtocolException("public key out of range")
        val lhs = y.multiply(y).mod(p)
        val rhs = x.pow(3).add(curve.a.multiply(x)).add(curve.b).mod(p)
        if (lhs != rhs) throw ProtocolException("public key not on P-256")
        return KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(ECPoint(x, y), p256))
    }

    /** ECDH shared secret: the 32-byte X coordinate. */
    fun ecdh(own: PrivateKey, peer: PublicKey): ByteArray {
        val ka = KeyAgreement.getInstance("ECDH")
        ka.init(own)
        ka.doPhase(peer, true)
        val z = ka.generateSecret()
        return if (z.size == 32) z else fixed32(BigInteger(1, z))
    }

    private fun fixed32(v: BigInteger): ByteArray {
        val b = v.toByteArray()
        return when {
            b.size == 32 -> b
            b.size == 33 && b[0].toInt() == 0 -> b.copyOfRange(1, 33)
            b.size < 32 -> ByteArray(32 - b.size) + b
            else -> throw IllegalArgumentException("value does not fit 32 bytes")
        }
    }

    // ------------------------------------------------------------ handshake formulas

    private val SAS_LABEL = "anydisplay/1 sas".toByteArray(Charsets.US_ASCII)
    private val KEYS_INFO = "anydisplay/1 keys".toByteArray(Charsets.US_ASCII)
    val HOST_LABEL = "host".toByteArray(Charsets.US_ASCII)
    val CLIENT_LABEL = "client".toByteArray(Charsets.US_ASCII)

    fun transcript(helloC: ByteArray, helloH: ByteArray, reveal: ByteArray) = sha256(helloC, helloH, reveal)

    fun sas(transcript: ByteArray): Int {
        val h = sha256(SAS_LABEL, transcript)
        val v = (h[0].toLong() and 0xFF) or ((h[1].toLong() and 0xFF) shl 8) or
            ((h[2].toLong() and 0xFF) shl 16) or ((h[3].toLong() and 0xFF) shl 24)
        return (v % 1_000_000L).toInt()
    }

    fun formatSas(code: Int) = "%06d".format(code)

    fun keySchedule(z: ByteArray, psk: ByteArray, transcript: ByteArray): SessionKeys {
        val okm = hkdfSha256(transcript, z + psk, KEYS_INFO, 96)
        return SessionKeys(okm.copyOfRange(0, 32), okm.copyOfRange(32, 64), okm.copyOfRange(64, 96))
    }

    fun confirmMac(kConfirm: ByteArray, label: ByteArray, transcript: ByteArray) = hmacSha256(kConfirm, label, transcript)
}

class SessionKeys(val kH2c: ByteArray, val kC2h: ByteArray, val kConfirm: ByteArray)

/**
 * AES-256-GCM per-frame cipher for one direction (PROTOCOL.md 3.1):
 * nonce = u64 seq LE || 4 zero bytes, aad = channel || type, 16-byte tag appended.
 */
class FrameCipher(key: ByteArray) {
    private val keySpec = SecretKeySpec(key, "AES")
    private val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    private val nonce = ByteArray(12)
    private val aad = ByteArray(2)
    var seq = 0L
        private set

    private fun prepare(mode: Int, channel: Int, type: Int) {
        val s = seq++
        for (i in 0 until 8) nonce[i] = (s ushr (8 * i)).toByte()
        aad[0] = channel.toByte(); aad[1] = type.toByte()
        cipher.init(mode, keySpec, GCMParameterSpec(128, nonce))
        cipher.updateAAD(aad)
    }

    fun encrypt(channel: Int, type: Int, body: ByteArray): ByteArray {
        prepare(Cipher.ENCRYPT_MODE, channel, type)
        return cipher.doFinal(body)
    }

    /** Throws on authentication failure; the caller closes the connection. */
    fun decrypt(channel: Int, type: Int, body: ByteArray): ByteArray {
        if (body.size < 16) throw ProtocolException("encrypted body shorter than tag")
        prepare(Cipher.DECRYPT_MODE, channel, type)
        return cipher.doFinal(body)
    }
}
