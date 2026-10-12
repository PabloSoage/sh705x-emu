package com.varuna.sh705x

/**
 * One calibration map as `crates/android` lays it out (`Live::map_data`), read in place:
 *
 * - [HEADER] numbers: dims (2 or 3), nx, ny (0 for a curve), the last lookup's X, Y and
 *   result, lookups, points in the trail;
 * - the X axis, the Y axis (none for a curve), the cells row by row (`z[j * nx + i]`);
 * - the trail: X, Y, result per lookup, oldest first.
 */
class MapData(private val raw: DoubleArray) {
    /** Whether the array has the shape its header announces. */
    val valid: Boolean
        get() = raw.size >= HEADER && raw.size == HEADER + nx + ny + nx * rows + 3 * trail

    /** 2 for a curve, 3 for a map. */
    val dims: Int get() = raw[0].toInt()

    /** Cells along X. */
    val nx: Int get() = raw[1].toInt()

    /** Cells along Y: 0 for a curve. */
    val ny: Int get() = raw[2].toInt()

    /** Rows of cells: 1 for a curve. */
    val rows: Int get() = maxOf(ny, 1)

    /** The last lookup's X. */
    val lastX: Double get() = raw[3]

    /** The last lookup's Y (NaN for a curve). */
    val lastY: Double get() = raw[4]

    /** The last lookup's result. */
    val lastOut: Double get() = raw[5]

    /** Lookups. */
    val calls: Long get() = raw[6].toLong()

    /** Points in the trail. */
    val trail: Int get() = raw[7].toInt()

    /** The X axis at [i]. */
    fun x(i: Int): Double = raw[HEADER + i]

    /** The Y axis at [j]. */
    fun y(j: Int): Double = raw[HEADER + nx + j]

    /** The cell at [i] along X and [j] along Y. */
    fun z(i: Int, j: Int): Double = raw[HEADER + nx + ny + j * nx + i]

    /** Trail point [k]: X, Y, result. */
    fun trailAt(k: Int): Triple<Double, Double, Double> {
        val at = HEADER + nx + ny + nx * rows + 3 * k
        return Triple(raw[at], raw[at + 1], raw[at + 2])
    }

    /** The lowest and highest cell. */
    fun span(): Pair<Double, Double> {
        var lo = Double.POSITIVE_INFINITY
        var hi = Double.NEGATIVE_INFINITY
        for (j in 0 until rows) for (i in 0 until nx) {
            val v = z(i, j)
            if (v < lo) lo = v
            if (v > hi) hi = v
        }
        return if (hi > lo) lo to hi else (lo - 1) to (hi + 1)
    }

    companion object {
        const val HEADER = 8

        /** Where [v] falls along an axis of [n] points read by [at], as a fractional index held to its ends. */
        fun position(n: Int, at: (Int) -> Double, v: Double): Double {
            if (n < 2 || v.isNaN() || v <= at(0)) return 0.0
            if (v >= at(n - 1)) return (n - 1).toDouble()
            var i = 0
            while (i < n - 2 && at(i + 1) <= v) i++
            val span = at(i + 1) - at(i)
            return i + if (span > 0) (v - at(i)) / span else 0.0
        }
    }
}
