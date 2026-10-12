package com.varuna.sh705x

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** A map as `crates/android` lays it out (`Live::map_data`, checked there by its own tests). */
class MapDataTest {
    /** A 3×2 map: X 0, 1000, 2000; Y 0, 50; z = 100 j + i; one trail point. */
    private fun map(): DoubleArray =
        doubleArrayOf(3.0, 3.0, 2.0, 1500.0, 25.0, 50.5, 7.0, 1.0) +
            doubleArrayOf(0.0, 1000.0, 2000.0) +
            doubleArrayOf(0.0, 50.0) +
            doubleArrayOf(0.0, 1.0, 2.0, 100.0, 101.0, 102.0) +
            doubleArrayOf(1500.0, 25.0, 50.5)

    @Test
    fun a_map_reads_axes_cells_and_trail() {
        val m = MapData(map())
        assertTrue(m.valid)
        assertEquals(3, m.dims)
        assertEquals(1000.0, m.x(1), 0.0)
        assertEquals(50.0, m.y(1), 0.0)
        assertEquals(102.0, m.z(2, 1), 0.0)
        assertEquals(Triple(1500.0, 25.0, 50.5), m.trailAt(0))
        assertEquals(0.0 to 102.0, m.span())
        assertEquals(7L, m.calls)
    }

    @Test
    fun a_curve_has_one_row_and_no_y_axis() {
        val c = MapData(doubleArrayOf(2.0, 2.0, 0.0, 5.0, Double.NaN, 3.0, 1.0, 0.0, 0.0, 10.0, 1.0, 5.0))
        assertTrue(c.valid)
        assertEquals(1, c.rows)
        assertEquals(5.0, c.z(1, 0), 0.0)
    }

    @Test
    fun positions_are_fractional_and_held_to_the_ends() {
        val axis = doubleArrayOf(0.0, 1000.0, 2000.0)
        assertEquals(1.5, MapData.position(3, { axis[it] }, 1500.0), 1e-9)
        assertEquals(0.0, MapData.position(3, { axis[it] }, -5.0), 0.0)
        assertEquals(2.0, MapData.position(3, { axis[it] }, 9999.0), 0.0)
    }

    @Test
    fun a_short_array_is_not_valid() {
        assertFalse(MapData(map().copyOf(10)).valid)
    }
}
