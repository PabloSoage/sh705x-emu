package com.varuna.sh705x

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** An oscilloscope frame as `crates/android` lays it out (`Live::osc_frame`). */
class OscFrameTest {
    @Test
    fun a_frame_reads_its_channels_and_edges() {
        val raw = doubleArrayOf(10_000.0, 2_000.0, 2.0, 1.0) +
            doubleArrayOf(0.0, 0.0, 2.0, 100.0, 1.0, 766.0, 0.0, 1333.3, 0.5) +
            doubleArrayOf(3.0, 1.0, 0.0, Double.NaN, Double.NaN)
        val f = OscFrame(raw)
        assertTrue(f.valid)
        assertTrue(f.triggered)
        assertEquals(10_000.0, f.spanUs, 0.0)
        assertEquals(2, f.traces.size)
        assertEquals(listOf(100.0 to true, 766.0 to false), f.traces[0].edges)
        assertEquals(1333.3, f.traces[0].periodUs, 0.0)
        assertTrue(f.traces[1].initial)
        assertTrue(f.traces[1].duty.isNaN())
    }

    @Test
    fun a_short_frame_is_not_valid() {
        assertFalse(OscFrame(doubleArrayOf(10_000.0, 0.0, 1.0, 0.0, 0.0, 0.0, 3.0)).valid)
        assertFalse(OscFrame(DoubleArray(0)).valid)
    }
}
