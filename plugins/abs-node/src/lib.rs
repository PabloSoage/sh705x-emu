//! The ABS on the bus: frame 0x1E9 every 21.4 ms, the one the ECU misses as U0121 (the
//! emulator's built-in "abs" peer, as a plugin), and every five seconds a line with the
//! frames the ECU has sent and its engine speed.

use sh705x_plugin::{Frame, log, manifest, read_f32, send};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

manifest!(
    r#"{"name": "ABS node", "version": "1.0.0", "api": 1,
        "description": "Sends the ABS frame 0x1E9 every 21.4 ms, and logs what the ECU sends",
        "reads": [["FFFEB7E4", "FFFEB7E8"]],
        "tick_us": 21400}"#
);

/// Frames the ECU has sent.
static HEARD: AtomicU32 = AtomicU32::new(0);
/// ECU time of the last log line, microseconds.
static LOGGED: AtomicU64 = AtomicU64::new(0);

/// Engine speed in the Astra's calibration (resultados fase 8).
const ENGINE_SPEED: u32 = 0xFFFE_B7E4;

#[unsafe(no_mangle)]
pub extern "C" fn init() {
    log("ABS node on the bus");
}

#[unsafe(no_mangle)]
pub extern "C" fn tick(now_us: i64) {
    send(
        &Frame::standard(0x1E9, &[0x0F, 0x98, 0x00, 0x0C, 0x0F, 0xFA, 0x00, 0x00]),
        0,
    );
    let now = now_us as u64;
    if now - LOGGED.load(Ordering::Relaxed) >= 5_000_000 {
        LOGGED.store(now, Ordering::Relaxed);
        let rpm = read_f32(ENGINE_SPEED).unwrap_or(f32::NAN);
        log(&format!(
            "{:.1} s: the ECU has sent {} frames; engine speed {rpm:.0} rpm",
            now as f64 / 1e6,
            HEARD.load(Ordering::Relaxed)
        ));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn on_frame(_id: i32, _flags: i32, _len: i32, _data: i64) {
    HEARD.fetch_add(1, Ordering::Relaxed);
}
