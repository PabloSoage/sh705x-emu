package com.varuna.sh705x

/**
 * One frame from the emulated ECU, laid out as `crates/android` builds it (`Live::frame`) and
 * read in place:
 *
 * - [HEADER] numbers: ECU time (µs), crank angle, rpm, real-time factor, running, speed,
 *   plants, the number of columns, the number of pulses;
 * - the columns, [COLUMN] numbers each: the time of the column's last sample, its crank
 *   angle, then each trace's minimum and maximum over the column, so a move faster than a
 *   column still shows;
 * - the channel 8 pulses: output, start, end (µs).
 */
class Frame(private val raw: DoubleArray) {
    /** Whether the array has the shape its header announces. */
    val valid: Boolean
        get() = raw.size >= HEADER && raw.size == HEADER + columns * COLUMN + pulses * 3

    /** ECU time, µs. */
    val nowUs: Double get() = raw[0]

    /** Crankshaft angle over 720°, null with the engine stopped. */
    val crank: Double? get() = raw[1].takeUnless { it.isNaN() }

    /** Engine speed, rpm. */
    val rpm: Double get() = raw[2]

    /** ECU time over wall time over the last second; 0 paused. */
    val realTime: Double get() = raw[3]

    /** Running or paused. */
    val running: Boolean get() = raw[4] != 0.0

    /** ECU time per wall-clock time. */
    val speed: Double get() = raw[5]

    /** The plants: [PLANTS_OFF], [PLANTS_HELD] or [PLANTS_TURNING]. */
    val plants: Int get() = raw[6].toInt()

    /** Columns of history. */
    val columns: Int get() = raw[7].toInt()

    /** Channel 8 pulses. */
    val pulses: Int get() = raw[8].toInt()

    /** Time of column [c]'s last sample, µs. */
    fun time(c: Int): Double = raw[HEADER + c * COLUMN]

    /** Crank angle at column [c]'s last sample, NaN stopped. */
    fun crankAt(c: Int): Double = raw[HEADER + c * COLUMN + 1]

    /** [trace]'s minimum over column [c], NaN where it has none. */
    fun min(c: Int, trace: Int): Double = raw[HEADER + c * COLUMN + 2 + 2 * trace]

    /** [trace]'s maximum over column [c], NaN where it has none. */
    fun max(c: Int, trace: Int): Double = raw[HEADER + c * COLUMN + 3 + 2 * trace]

    /** [trace] in the last column (its maximum there), NaN if none. */
    fun last(trace: Int): Double = if (columns == 0) Double.NaN else max(columns - 1, trace)

    /** Pulse [i]: output (0 = TO8A .. 15 = TO8P), start and end, µs. */
    fun pulse(i: Int): Pulse {
        val at = HEADER + columns * COLUMN + 3 * i
        return Pulse(raw[at].toInt(), raw[at + 1], raw[at + 2])
    }

    /** A one-shot pulse of the ATU-II's channel 8. */
    data class Pulse(val output: Int, val startUs: Double, val endUs: Double)

    companion object {
        /** Traces in a column, in `session::scope::TRACES` order. */
        const val TRACES = 15
        const val HEADER = 9
        const val COLUMN = 2 + 2 * TRACES

        const val PLANTS_OFF = 0
        const val PLANTS_HELD = 1
        const val PLANTS_TURNING = 2

        /** Nothing running yet. */
        val EMPTY = Frame(DoubleArray(HEADER) { if (it == 1) Double.NaN else 0.0 })
    }
}
