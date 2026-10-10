//! `sh705x-gui`: watch the emulated ECU run. Registers, the code around the PC with
//! breakpoints, memory, CAN traffic, interrupts, the engine (speed and sensors in
//! physical units, and the raw A/D codes) as sliders, and an ELM327 console on the
//! emulated CAN bus.
//!
//! ```text
//! sh705x-gui --flash <flash.bin> [--eeprom <eeprom.bin>] [--eeprom-layout 0123[le]]
//! ```

mod app;
mod emu;

use sh7059::pins::{Merge, Pin, SquareWave};
use sh7059::{Machine, Sh7059};
use std::sync::{Arc, Mutex, mpsc};

/// AN14 code that turns the ignition on (see `sh705x` IGNITION_ON).
const IGNITION_ON: u16 = 600;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut flash, mut eeprom, mut layout) = (None, None, board::DEFAULT_LAYOUT);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let v = it.next().ok_or_else(|| format!("{a} needs a value"))?;
        match a.as_str() {
            "--flash" => flash = Some(v.clone()),
            "--eeprom" => eeprom = Some(v.clone()),
            "--eeprom-layout" => {
                let (digits, le) = v
                    .strip_suffix("le")
                    .map_or((v.as_str(), false), |d| (d, true));
                let b: Vec<usize> = digits
                    .chars()
                    .filter_map(|c| c.to_digit(4).map(|d| d as usize))
                    .collect();
                if b.len() != 4 {
                    return Err(format!("bad --eeprom-layout '{v}'"));
                }
                layout = board::Layout {
                    blocks: [b[0], b[1], b[2], b[3]],
                    big_endian: !le,
                };
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    let flash = flash.ok_or("usage: sh705x-gui --flash <flash.bin> [--eeprom <eeprom.bin>]")?;
    let image = std::fs::read(&flash).map_err(|e| format!("{flash}: {e}"))?;
    let mut chip = Sh7059::new(&image).map_err(|e| format!("{flash}: {e}"))?;
    if let Some(path) = eeprom {
        let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
        let e = board::Eeprom::new(&bytes).map_err(|e| format!("{path}: {e}"))?;
        if e.len() != 4 * board::CHIP_BYTES {
            return Err(format!("{path}: the board has four 93C86, 8192 bytes"));
        }
        board::wire_eeproms(&mut chip, &e, layout);
    }
    chip.adc.input[14] = IGNITION_ON;
    // The crank and cam wheels (stopped) and the MAF, so both can be set while running.
    chip.set_stimulus(Box::new(Merge(vec![
        Box::new(engine::wheel::Wheels::new(0.0, 0)),
        Box::new(SquareWave::new(
            Pin::Ti0(2),
            engine::sensors::MAF_KEY,
            0.0,
            engine::wheel::CYCLES_PER_S,
            0,
        )),
    ])));
    chip.set_can_source(Box::new(engine::bus::abs()));
    let mut m = Machine::new(chip, true);
    let engine = emu::Engine::ignition_on();
    engine.apply(&mut m);
    let shared = Arc::new(Mutex::new(emu::first_snapshot(&m, &engine)));
    let (tx, rx) = mpsc::channel();
    let s2 = shared.clone();
    let thread = std::thread::spawn(move || emu::run(m, engine, rx, s2));
    let flash = Arc::new(image);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_title("SH705x emulator"),
        ..Default::default()
    };
    let tx2 = tx.clone();
    let r = eframe::run_native(
        "sh705x-gui",
        options,
        Box::new(move |_| Ok(Box::new(app::App::new(shared, tx2, flash)))),
    );
    let _ = tx.send(emu::Cmd::Quit);
    let _ = thread.join();
    r.map_err(|e| e.to_string())
}
