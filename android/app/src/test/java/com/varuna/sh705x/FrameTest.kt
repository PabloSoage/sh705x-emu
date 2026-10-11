package com.varuna.sh705x

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The frame as `crates/android` lays it out (`Live::frame`, checked there by its own tests). */
class FrameTest {
    private fun frame(columns: List<DoubleArray>, pulses: List<DoubleArray>, crank: Double = 363.0): DoubleArray {
        val header = doubleArrayOf(
            2_000_000.0, crank, 750.0, 0.98, 1.0, 0.01, 2.0,
            columns.size.toDouble(), pulses.size.toDouble(),
        )
        return header + columns.fold(DoubleArray(0)) { a, c -> a + c } + pulses.fold(DoubleArray(0)) { a, p -> a + p }
    }

    private fun column(us: Double, base: Double): DoubleArray =
        doubleArrayOf(us, 100.0) + DoubleArray(2 * Frame.TRACES) { base + it / 2 + (it % 2) }

    @Test
    fun the_header_reads_as_the_native_side_writes_it() {
        val f = Frame(frame(listOf(column(1.0, 0.0)), emptyList()))
        assertTrue(f.valid)
        assertEquals(2_000_000.0, f.nowUs, 0.0)
        assertEquals(363.0, f.crank!!, 0.0)
        assertEquals(750.0, f.rpm, 0.0)
        assertTrue(f.running)
        assertEquals(0.01, f.speed, 0.0)
        assertEquals(Frame.PLANTS_TURNING, f.plants)
    }

    @Test
    fun columns_hold_each_trace_minimum_then_maximum() {
        val f = Frame(frame(listOf(column(10.0, 0.0), column(20.0, 100.0)), emptyList()))
        assertEquals(2, f.columns)
        assertEquals(20.0, f.time(1), 0.0)
        assertEquals(100.0, f.crankAt(0), 0.0)
        // Trace 3 in column 1: minimum 100 + 3, maximum 100 + 3 + 1.
        assertEquals(103.0, f.min(1, 3), 0.0)
        assertEquals(104.0, f.max(1, 3), 0.0)
        assertEquals(f.max(1, 14), f.last(14), 0.0)
    }

    @Test
    fun pulses_follow_the_columns() {
        val f = Frame(frame(listOf(column(10.0, 0.0)), listOf(doubleArrayOf(9.0, 5.0, 7.5))))
        assertTrue(f.valid)
        assertEquals(Frame.Pulse(9, 5.0, 7.5), f.pulse(0))
    }

    @Test
    fun a_stopped_engine_has_no_crank_angle_and_a_short_array_is_not_valid() {
        assertNull(Frame(frame(emptyList(), emptyList(), crank = Double.NaN)).crank)
        assertFalse(Frame(frame(listOf(column(1.0, 0.0)), emptyList()).copyOf(20)).valid)
        assertTrue(Frame.EMPTY.valid)
    }
}
