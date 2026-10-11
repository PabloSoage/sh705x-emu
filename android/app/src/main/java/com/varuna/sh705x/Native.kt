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
}
