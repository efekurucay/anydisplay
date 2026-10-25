package dev.anydisplay.client.input

import kotlin.math.PI
import kotlin.math.asin
import kotlin.math.cos
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.math.sqrt

object PenMath {
    /**
     * Converts Android stylus axes to Windows-convention tilt (PROTOCOL.md 5.5), in hundredths of a degree.
     *
     * Android's input reader derives its axes from the device's tilt_x/tilt_y as
     * `orientation = atan2(-sin(tx), sin(ty))` and `tilt = acos(cos(tx) * cos(ty))`. This function inverts that
     * mapping exactly: with a = sin(tx) = -r·sin(φ), b = sin(ty) = r·cos(φ), the identity
     * cos²θ = (1 - a²)(1 - b²) gives s·u² - u + sin²θ = 0 for u = r², s = sin²φ·cos²φ.
     *
     * @param tilt AXIS_TILT in radians (0 = perpendicular to the screen).
     * @param orientation AXIS_ORIENTATION in radians (0 = pointing up, π/2 = pointing right).
     */
    fun tiltCentidegrees(tilt: Float, orientation: Float): Pair<Int, Int> {
        val theta = tilt.toDouble().coerceIn(0.0, PI / 2)
        if (theta < 1e-6) return 0 to 0
        val phi = orientation.toDouble()
        val sinPhi = sin(phi)
        val cosPhi = cos(phi)
        val sin2Theta = sin(theta) * sin(theta)
        val s = sinPhi * sinPhi * cosPhi * cosPhi
        val u = if (s < 1e-9) sin2Theta else (1 - sqrt((1 - 4 * s * sin2Theta).coerceAtLeast(0.0))) / (2 * s)
        val r = sqrt(u.coerceIn(0.0, 2.0))
        val tx = asin((-r * sinPhi).coerceIn(-1.0, 1.0))
        val ty = asin((r * cosPhi).coerceIn(-1.0, 1.0))
        return toCenti(tx) to toCenti(ty)
    }

    private fun toCenti(rad: Double) = (rad * 18000.0 / PI).roundToInt().coerceIn(-9000, 9000)

    /** Android's forward mapping, used by tests to check [tiltCentidegrees] round-trips. */
    fun androidAxes(tiltXRad: Double, tiltYRad: Double): Pair<Double, Double> {
        val orientation = kotlin.math.atan2(-sin(tiltXRad), sin(tiltYRad))
        val tilt = kotlin.math.acos(cos(tiltXRad) * cos(tiltYRad))
        return tilt to orientation
    }
}
