//! `sh705x maps`: what the ECU asks for across engine speed and pedal, read back over
//! diagnostics, for one flash image or two side by side.
//!
//! For each point of the grid the ECU boots with the engine at that speed and the pedal
//! at that position, the other sensors at warm idle, and is asked for its setpoints:
//! desired rail pressure, boost and air flow, load, desired EGR and throttle. They come
//! from the calibration's maps, so two images that differ in a map differ here where the
//! engine is on it.
//!
//! Each point is a steady state: a first pass, with the manifold at the barometric
//! pressure, gives the desired boost; the next puts the boost sensor there, as if the
//! turbo had reached it, with the MAF reading what the engine draws at that speed and
//! pressure (`engine::air`), until the desired boost stops moving (up to four passes). The ECU does not inject
//! (see the README), so these are the values it asks for, not what an engine would
//! reach.

use crate::{EcuOptions, boot};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// A setpoint read over GMLAN service 22.
struct Setpoint {
    did: u16,
    name: &'static str,
    unit: &'static str,
    bytes: usize,
    scale: f64,
    /// Two's-complement: torque is negative on the overrun.
    signed: bool,
}

/// What is read at each point. Fuel, rail and air come first (the heart of a remap),
/// then the actuator commands they drive, then EGR and throttle. The identifiers and
/// their scales are the ones the car's own route profile polls
/// (`opendash-plugins/opel-vauxhall/profiles.txt`), so the ECU answers them.
const SETPOINTS: &[Setpoint] = &[
    Setpoint {
        did: 0x3233,
        name: "fuel per stroke",
        unit: "mm3",
        bytes: 2,
        scale: 0.1,
        signed: false,
    },
    Setpoint {
        did: 0x1A2D,
        name: "torque delivered",
        unit: "N·m",
        bytes: 2,
        scale: 0.25,
        signed: true,
    },
    Setpoint {
        did: 0x308A,
        name: "desired rail pressure",
        unit: "MPa",
        bytes: 2,
        scale: 0.01,
        signed: false,
    },
    Setpoint {
        did: 0x323E,
        // Despite its name in the car's profile ("regulator command"), this reads back
        // whatever current is set on AN1: the measured current [MED].
        name: "SCV current (measured)",
        unit: "mA",
        bytes: 2,
        scale: 1.0,
        signed: false,
    },
    Setpoint {
        did: 0x323D,
        name: "SCV duty command",
        unit: "%",
        bytes: 1,
        scale: 100.0 / 255.0,
        signed: false,
    },
    Setpoint {
        did: 0x327B,
        name: "desired air flow",
        unit: "g/s",
        bytes: 2,
        scale: 0.01,
        signed: false,
    },
    Setpoint {
        did: 0x328A,
        name: "desired boost pressure",
        unit: "kPa",
        bytes: 2,
        scale: 0.01,
        signed: false,
    },
    Setpoint {
        did: 0x2041,
        name: "VGT vane command",
        unit: "%",
        bytes: 1,
        scale: 100.0 / 255.0,
        signed: false,
    },
    Setpoint {
        did: 0x002C,
        name: "desired EGR position",
        unit: "%",
        bytes: 1,
        scale: 100.0 / 255.0,
        signed: false,
    },
    Setpoint {
        did: 0x32A8,
        name: "EGR command",
        unit: "%",
        bytes: 1,
        scale: 1.0,
        signed: false,
    },
    Setpoint {
        did: 0x004C,
        name: "desired throttle position",
        unit: "%",
        bytes: 1,
        scale: 100.0 / 255.0,
        signed: false,
    },
    Setpoint {
        did: 0x0004,
        name: "engine load",
        unit: "%",
        bytes: 1,
        scale: 100.0 / 255.0,
        signed: false,
    },
];

/// Options of `sh705x maps`.
#[derive(Debug, PartialEq, Eq)]
pub struct MapsOptions {
    /// The ECU, with the first image.
    pub ecu: EcuOptions,
    /// A second image to compare with the first.
    pub compare: Option<PathBuf>,
    /// Engine speeds, rpm.
    pub rpm: Vec<u32>,
    /// Pedal positions, %.
    pub pedal: Vec<u32>,
}

/// The setpoints at one point, in setpoint order; `None` where the ECU did not answer.
fn read_point(o: &EcuOptions) -> Result<Vec<Option<f64>>, String> {
    let mut m = boot(o)?;
    let mut link = bridge::link::VirtualLink::new(&mut m, 0);
    let mut elm = bridge::elm::Elm327::new();
    elm.echo = false;
    Ok(SETPOINTS
        .iter()
        .map(|s| {
            let answer = elm.line(&format!("22{:04X}", s.did), &mut link);
            let b: Vec<u8> = answer
                .split_whitespace()
                .filter_map(|h| u8::from_str_radix(h, 16).ok())
                .collect();
            let echo = [0x62, (s.did >> 8) as u8, s.did as u8];
            let at = b.windows(3).position(|w| w == echo)? + 3;
            let raw = b.get(at..at + s.bytes)?;
            let v = raw.iter().fold(0u32, |a, &x| a << 8 | x as u32);
            let v = if s.signed && raw[0] & 0x80 != 0 {
                // Sign-extend from the field's width (1 or 2 bytes).
                (v | !((1 << (8 * s.bytes)) - 1)) as i32 as f64
            } else {
                v as f64
            };
            Some(v * s.scale)
        })
        .collect())
}

/// The ECU at `rpm` with the pedal at `pedal` %, on `flash`, with the manifold at
/// `boost` kPa and the air the engine draws there.
fn point(o: &EcuOptions, flash: &Path, rpm: u32, pedal: u32, boost: f64) -> EcuOptions {
    let mut p = o.clone();
    p.flash = flash.to_path_buf();
    p.rpm = Some(rpm);
    p.sensor.push(format!("pedal={pedal}"));
    p.sensor.push(format!("boost={boost}"));
    let maf = engine::air::maf_g_per_s(rpm as f64, boost, 26.0);
    p.sensor.push(format!("maf={maf}"));
    p
}

/// Index of the desired boost in [`SETPOINTS`].
const DESIRED_BOOST: usize = 6;

/// The setpoints at one point once the engine has settled: a first pass at the
/// barometric pressure gives the desired boost; the next puts the manifold there, as
/// if the turbo reached its target, and reads them again, until the desired boost moves
/// less than 1 kPa or after [`PASSES`] passes.
fn settled(o: &EcuOptions, flash: &Path, rpm: u32, pedal: u32) -> Result<Vec<Option<f64>>, String> {
    let mut boost = BARO;
    let mut values = read_point(&point(o, flash, rpm, pedal, boost))?;
    for _ in 1..PASSES {
        let want = values[DESIRED_BOOST].unwrap_or(BARO).max(BARO);
        if (want - boost).abs() < 1.0 {
            break;
        }
        boost = want;
        values = read_point(&point(o, flash, rpm, pedal, boost))?;
    }
    Ok(values)
}

/// Most passes for a point to settle.
const PASSES: usize = 4;

/// Barometric pressure of the defaults, kPa.
const BARO: f64 = 98.0;

/// Runs the grid on one image: values[setpoint][rpm][pedal].
fn grid(o: &MapsOptions, flash: &Path) -> Result<Vec<Vec<Vec<Option<f64>>>>, String> {
    let cells: Vec<(usize, usize)> = (0..o.rpm.len())
        .flat_map(|r| (0..o.pedal.len()).map(move |p| (r, p)))
        .collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let results: Vec<Result<Vec<Option<f64>>, String>> = std::thread::scope(|s| {
        let chunks: Vec<_> = cells
            .chunks(cells.len().div_ceil(threads).max(1))
            .map(|chunk| {
                s.spawn(move || {
                    chunk
                        .iter()
                        .map(|&(r, p)| settled(&o.ecu, flash, o.rpm[r], o.pedal[p]))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        chunks
            .into_iter()
            .flat_map(|h| h.join().expect("grid thread"))
            .collect()
    });
    let mut out = vec![vec![vec![None; o.pedal.len()]; o.rpm.len()]; SETPOINTS.len()];
    for (&(r, p), res) in cells.iter().zip(results) {
        for (k, v) in res?.into_iter().enumerate() {
            out[k][r][p] = v;
        }
    }
    Ok(out)
}

fn table(out: &mut String, o: &MapsOptions, title: &str, cell: impl Fn(usize, usize) -> String) {
    writeln!(out, "{title}").unwrap();
    write!(out, "{:>10}", "rpm\\pedal").unwrap();
    for p in &o.pedal {
        write!(out, "{:>9}", format!("{p} %")).unwrap();
    }
    writeln!(out).unwrap();
    for (r, rpm) in o.rpm.iter().enumerate() {
        write!(out, "{rpm:>10}").unwrap();
        for p in 0..o.pedal.len() {
            write!(out, "{:>9}", cell(r, p)).unwrap();
        }
        writeln!(out).unwrap();
    }
    writeln!(out).unwrap();
}

fn show(v: Option<f64>) -> String {
    v.map_or("-".into(), |v| format!("{v:.2}"))
}

/// Runs `sh705x maps`.
pub fn maps(o: &MapsOptions) -> Result<String, String> {
    let a = grid(o, &o.ecu.flash)?;
    let b = o.compare.as_ref().map(|f| grid(o, f)).transpose()?;
    let mut out = String::new();
    let mut differ = 0;
    for (k, s) in SETPOINTS.iter().enumerate() {
        let title = format!("{} ({}), identifier {:04X}", s.name, s.unit, s.did);
        match &b {
            None => table(&mut out, o, &title, |r, p| show(a[k][r][p])),
            Some(b) => {
                let same =
                    (0..o.rpm.len()).all(|r| (0..o.pedal.len()).all(|p| a[k][r][p] == b[k][r][p]));
                if same {
                    writeln!(out, "{title}: the same at every point\n").unwrap();
                    continue;
                }
                differ += 1;
                table(&mut out, o, &format!("{title}, first image"), |r, p| {
                    show(a[k][r][p])
                });
                table(
                    &mut out,
                    o,
                    &format!("{title}, second minus first"),
                    |r, p| match (a[k][r][p], b[k][r][p]) {
                        (Some(x), Some(y)) if x == y => ".".into(),
                        (Some(x), Some(y)) => format!("{:+.2}", y - x),
                        _ => "?".into(),
                    },
                );
            }
        }
    }
    if b.is_some() {
        writeln!(
            out,
            "{differ} of {} setpoints differ between the images.",
            SETPOINTS.len()
        )
        .unwrap();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{DESIRED_BOOST, SETPOINTS};

    #[test]
    fn desired_boost_points_at_its_setpoint() {
        // `settled` feeds this setpoint's value back as the manifold pressure, so the
        // index must track the table if it is reordered.
        assert_eq!(SETPOINTS[DESIRED_BOOST].did, 0x328A);
    }
}
