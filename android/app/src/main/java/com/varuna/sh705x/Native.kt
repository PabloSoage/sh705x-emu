package com.varuna.sh705x

/**
 * The emulated ECU, in `crates/android` (`libsh705x_jni.so`).
 *
 * It runs on a thread of its own on the native side; every call here only sends it an order
 * or reads what it published last, so none of them blocks for long and all can be made from
 * the main thread. [open] is the exception, as it copies the images: call it off the main
 * thread.
 */
object Native {
    init {
        System.loadLibrary("sh705x_jni")
    }

    /** Powers on an ECU on [flash] and [eeprom] (null for none), paused: "" or what is wrong. */
    external fun open(flash: ByteArray, eeprom: ByteArray?): String

    /** Ends the running ECU. */
    external fun close()

    /** The traces, one per line: name, a tab, unit (the same order as [Frame]'s). */
    external fun traces(): String

    /** The status and the last [windowUs] of history in up to [columns] columns: see [Frame]. */
    external fun frame(windowUs: Double, columns: Int): DoubleArray

    /** Why the ECU last stopped other than by being paused, or "". */
    external fun stop(): String

    /** Runs or pauses. */
    external fun run(run: Boolean)

    /** ECU time per wall-clock time: 1 is real time, 0.001 a tooth at idle about every second. */
    external fun speed(speed: Double)

    /** The engine speed, rpm (0 stops it); with the engine on its torque, where it goes on from. */
    external fun rpm(rpm: Double)

    /** A sensor by key, in physical units (`pedal` in % of its travel). */
    external fun sensor(key: String, value: Double)

    /** The plants: [Frame.PLANTS_OFF], [Frame.PLANTS_HELD] or [Frame.PLANTS_TURNING]. */
    external fun plants(mode: Int)

    /** All sensors and the speed at once: ignition on and still (false) or idling. */
    external fun preset(idle: Boolean)

    /** Paused, runs to the next tooth position (6°). */
    external fun nextTooth()

    /** Paused, runs to the next edge on the input pins (crank, cam or MAF). */
    external fun nextEdge()

    /** Traces the calibration maps the firmware reads, or stops (about 6 % slower while on). */
    external fun traceMaps(on: Boolean)

    /** Stops at the next lookup of map [desc], or no longer (negative). */
    external fun breakOnMap(desc: Int)

    /** How the map trace stands, in one line; empty while the maps are not traced. */
    external fun mapsStatus(): String

    /**
     * The maps read, the latest first, those [filter] matches: one per line,
     * `desc dims nx ny lookups last name`, tab-separated.
     */
    external fun mapList(filter: String): String

    /** One map's numbers: see [MapData]. Empty if the map is not known. */
    external fun mapData(desc: Int): DoubleArray

    /** What is known about a map, in words, one fact per line. */
    external fun mapText(desc: Int): String

    /** The maps a map's inputs come from and that read its result, one hex descriptor per line. */
    external fun mapSources(desc: Int): String

    /** Names the variables the maps read by moving the sensors (a few seconds). */
    external fun nameSensors()

    /** The profiles in an opendash profile file, one name per line. */
    external fun profileNames(text: String): String

    /** Names the variables from profile [index] of an opendash profile file, and the sensors. */
    external fun nameFromProfile(text: String, index: Int)

    /** Adds the map labels in a label file; returns how many. */
    external fun labelMaps(text: String): Int

    /** The fault codes in one line; empty while they are not traced (with the maps) or not known. */
    external fun codesStatus(): String

    /**
     * The codes [filter] matches, only failing or disabled ones if asked: one per line,
     * `index code enabled fails tests last status text`, tab-separated.
     */
    external fun codeList(filter: String, failing: Boolean, disabled: Boolean): String

    /** One code in words, one fact per line. */
    external fun codeText(index: Int): String

    /** The maps a code's tests read, one hex descriptor per line. */
    external fun codeMaps(index: Int): String

    /** Stops at the next test of code [index], or no longer (negative). */
    external fun breakOnTest(index: Int)

    /** Asks the ECU for its confirmed and pending codes. */
    external fun readCodes()

    /** Adds code descriptions from a file (a code, then its text, per line); returns how many. */
    external fun describeCodes(text: String): Int
}
