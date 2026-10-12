package com.varuna.sh705x

/**
 * One oscilloscope frame as `crates/android` lays it out (`Live::osc_frame`): the window's
 * span (µs), the trigger's time from its start (NaN running free), the channels, whether it
 * is held on a trigger; then per channel its index, its level at the start, its edges as
 * (time from the start in µs, level), and its period (µs) and duty (NaN without two
 * rising edges).
 */
class OscFrame(raw: DoubleArray) {
    /** One channel over the window. */
    data class Trace(
        val channel: Int,
        val initial: Boolean,
        val edges: List<Pair<Double, Boolean>>,
        val periodUs: Double,
        val duty: Double,
    )

    /** The window, µs. */
    val spanUs: Double

    /** The trigger, µs from the start; NaN running free. */
    val triggerUs: Double

    /** Held on a trigger. */
    val triggered: Boolean

    /** The channels, in the order asked for. */
    val traces: List<Trace>

    /** Whether the array read as a frame. */
    val valid: Boolean

    init {
        var ok = raw.size >= HEADER
        val traces = ArrayList<Trace>()
        var at = HEADER
        if (ok) {
            repeat(raw[2].toInt()) {
                if (at + 3 > raw.size) {
                    ok = false
                    return@repeat
                }
                val ch = raw[at].toInt()
                val initial = raw[at + 1] != 0.0
                val n = raw[at + 2].toInt()
                at += 3
                if (at + 2 * n + 2 > raw.size) {
                    ok = false
                    return@repeat
                }
                val edges = (0 until n).map { raw[at + 2 * it] to (raw[at + 2 * it + 1] != 0.0) }
                at += 2 * n
                traces += Trace(ch, initial, edges, raw[at], raw[at + 1])
                at += 2
            }
        }
        valid = ok && at == raw.size
        spanUs = if (ok) raw[0] else 0.0
        triggerUs = if (ok) raw[1] else Double.NaN
        triggered = ok && raw[3] != 0.0
        this.traces = traces
    }

    companion object {
        const val HEADER = 4
    }
}
