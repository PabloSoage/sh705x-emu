//! `sh705x maps`: what the ECU asks for across engine speed and pedal, read back over
//! diagnostics, for one flash image or two side by side.
//!
//! For each point of the grid the ECU boots with the engine at that speed and the pedal
//! at that position, the other sensors at warm idle, and is asked for its setpoints:
//! desired rail pressure, boost and air flow, load, desired EGR and throttle. They come
//! from the calibration's maps, so two images that differ in a map differ here where the
//! engine is on it.
//!
//! Each point is a steady state: the engine is held at the point's speed, as on a dyno,
//! while the ECU boots with the plants around it (`crate::rig`): the EGR, the fresh air,
//! the SCV and the turbocharger close their loops before the setpoints are read. The ECU
//! does not inject (see the README), so these are the values it asks for, not what an
//! engine would reach; the fuel it asks for is read from RAM.

use crate::plant::QUANTITY_AT;
use crate::rig::Rig;
use crate::{EcuOptions, f32_at};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Where a setpoint is read.
enum From {
    /// A GMLAN identifier, over service 22.
    Did(u16),
    /// A float in RAM, for what the diagnostics do not report.
    Ram(u32),
}

/// A setpoint.
struct Setpoint {
    from: From,
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
        from: From::Did(0x3233),
        name: "fuel per stroke",
        unit: "mm3",
        bytes: 2,
        scale: 0.1,
        signed: false,
    },
    Setpoint {
        // What the firmware asks for: 0x3233 is what it injects, 0 while the immobiliser
        // holds it shut (resultados fase 8).
        from: From::Ram(QUANTITY_AT),
        name: "fuel asked for",
        unit: "mm3",
        bytes: 4,
        scale: 1.0,
        signed: false,
    },
    Setpoint {
        from: From::Did(0x1A2D),
        name: "torque delivered",
        unit: "N·m",
        bytes: 2,
        scale: 0.25,
        signed: true,
    },
    Setpoint {
        from: From::Did(0x308A),
        name: "desired rail pressure",
        unit: "MPa",
        bytes: 2,
        scale: 0.01,
        signed: false,
    },
    Setpoint {
        from: From::Did(0x323E),
        // Despite its name in the car's profile ("regulator command"), this reads back
        // whatever current is set on AN1: the measured current [MED].
        name: "SCV current (measured)",
        unit: "mA",
        bytes: 2,
        scale: 1.0,
        signed: false,
    },
    Setpoint {
        from: From::Did(0x323D),
        name: "SCV duty command",
        unit: "%",
        bytes: 1,
        scale: 100.0 / 255.0,
        signed: false,
    },
    Setpoint {
        from: From::Did(0x327B),
        name: "desired air flow",
        unit: "g/s",
        bytes: 2,
        scale: 0.01,
        signed: false,
    },
    Setpoint {
        from: From::Did(0x328A),
        name: "desired boost pressure",
        unit: "kPa",
        bytes: 2,
        scale: 0.01,
        signed: false,
    },
    Setpoint {
        from: From::Did(0x2041),
        name: "VGT vane command",
        unit: "%",
        bytes: 1,
        scale: 100.0 / 255.0,
        signed: false,
    },
    Setpoint {
        from: From::Did(0x002C),
        name: "desired EGR position",
        unit: "%",
        bytes: 1,
        scale: 100.0 / 255.0,
        signed: false,
    },
    Setpoint {
        from: From::Did(0x32A8),
        name: "EGR command",
        unit: "%",
        bytes: 1,
        scale: 1.0,
        signed: true,
    },
    Setpoint {
        from: From::Did(0x004C),
        name: "desired throttle position",
        unit: "%",
        bytes: 1,
        scale: 100.0 / 255.0,
        signed: false,
    },
    Setpoint {
        from: From::Did(0x0004),
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
/// The engine turns at the point's speed, held there as on a dyno, with the plants of
/// [`crate::rig`] around the ECU from power-on: the EGR and the fresh air it leaves, the
/// SCV and the turbocharger close their loops while it boots, so what is read is the
/// steady state the firmware reaches there.
fn read_point(o: &EcuOptions) -> Result<Vec<Option<f64>>, String> {
    let mut rig = Rig::new(o, None, 1000)?;
    let quantity = f32_at(&rig.m, QUANTITY_AT);
    let mut link = bridge::link::VirtualLink::new(&mut rig.m, 0);
    let mut elm = bridge::elm::Elm327::new();
    elm.echo = false;
    Ok(SETPOINTS
        .iter()
        .map(|s| {
            let did = match s.from {
                From::Ram(_) => return Some(quantity),
                From::Did(did) => did,
            };
            let answer = elm.line(&format!("22{did:04X}"), &mut link);
            let b: Vec<u8> = answer
                .split_whitespace()
                .filter_map(|h| u8::from_str_radix(h, 16).ok())
                .collect();
            let echo = [0x62, (did >> 8) as u8, did as u8];
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

/// The ECU at `rpm` with the pedal at `pedal` %, on `flash`.
fn point(o: &EcuOptions, flash: &Path, rpm: u32, pedal: u32) -> EcuOptions {
    let mut p = o.clone();
    p.flash = flash.to_path_buf();
    p.rpm = Some(rpm);
    p.sensor.push(format!("pedal={pedal}"));
    p
}

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
                        .map(|&(r, p)| read_point(&point(&o.ecu, flash, o.rpm[r], o.pedal[p])))
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

/// A one-line summary of a grid: empty, all zero, constant, or the range it spans. It keeps
/// a reader from drawing conclusions from a setpoint that is 0 or flat across the whole grid
/// (as a comparison of two all-zero grids once did).
fn span(g: &[Vec<Option<f64>>]) -> String {
    let (mut min, mut max, mut any) = (f64::INFINITY, f64::NEG_INFINITY, false);
    for v in g.iter().flatten().flatten() {
        any = true;
        min = min.min(*v);
        max = max.max(*v);
    }
    if !any {
        "no values".into()
    } else if min == max {
        if min == 0.0 {
            "all zero".into()
        } else {
            format!("constant at {min:.2}")
        }
    } else {
        format!("range {min:.2}..{max:.2}")
    }
}

/// Runs `sh705x maps`.
pub fn maps(o: &MapsOptions) -> Result<String, String> {
    let a = grid(o, &o.ecu.flash)?;
    let b = o.compare.as_ref().map(|f| grid(o, f)).transpose()?;
    let mut out = String::new();
    let mut differ = 0;
    for (k, s) in SETPOINTS.iter().enumerate() {
        let source = match s.from {
            From::Did(did) => format!("identifier {did:04X}"),
            From::Ram(at) => format!("RAM {at:08X}"),
        };
        let title = format!("{} ({}), {source}", s.name, s.unit);
        match &b {
            None => {
                table(&mut out, o, &title, |r, p| show(a[k][r][p]));
                writeln!(out, "{}: {}\n", s.name, span(&a[k])).unwrap();
            }
            Some(b) => {
                let same =
                    (0..o.rpm.len()).all(|r| (0..o.pedal.len()).all(|p| a[k][r][p] == b[k][r][p]));
                if same {
                    writeln!(out, "{title}: the same at every point ({})\n", span(&a[k])).unwrap();
                    continue;
                }
                differ += 1;
                table(&mut out, o, &format!("{title}, first image"), |r, p| {
                    show(a[k][r][p])
                });
                writeln!(
                    out,
                    "first image: {}; second image: {}\n",
                    span(&a[k]),
                    span(&b[k])
                )
                .unwrap();
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

/// Options of the `replay` command.
#[derive(Debug, PartialEq, Eq)]
pub struct ReplayOptions {
    /// The ECU, with the firmware under test.
    pub ecu: EcuOptions,
    /// The cells CSV, summarised from a recorded route by `tools/route_cells.py`.
    pub cells: PathBuf,
}

/// The inputs a cell drives, as (CSV column, sensor key).
const DRIVEN: &[(&str, &str)] = &[
    ("pedal", "pedal"),
    ("coolant", "coolant"),
    ("intake", "intake"),
    ("boost", "boost"),
    ("baro", "baro"),
    ("maf", "maf"),
];

/// The setpoints compared, as (CSV column, the identifier the firmware answers for it).
/// These are the ones that move without injection (the firmware computes them from speed,
/// load and the plants), so a logged route can check them; `fuel per stroke` cannot.
const COMPARED: &[(&str, u16)] = &[
    ("rail", 0x308A),
    ("boost_cmd", 0x328A),
    ("vgt", 0x2041),
    ("air", 0x327B),
    ("torque", 0x1A2D),
];

/// The `SETPOINTS` index that answers identifier `did`.
fn setpoint_of(did: u16) -> Option<usize> {
    SETPOINTS
        .iter()
        .position(|s| matches!(s.from, From::Did(d) if d == did))
}

/// A recorded operating point: the inputs to drive, and the setpoints the car logged there.
struct Cell {
    rpm: u32,
    driven: Vec<(&'static str, f64)>,
    recorded: Vec<Option<f64>>,
    n: u64,
}

/// The ECU on `flash` at a cell's recorded inputs.
fn at_cell(o: &EcuOptions, flash: &Path, c: &Cell) -> EcuOptions {
    let mut p = o.clone();
    p.flash = flash.to_path_buf();
    p.rpm = Some(c.rpm);
    for (key, v) in &c.driven {
        p.sensor.push(format!("{key}={v}"));
    }
    p
}

/// Runs `sh705x replay`: drive each cell's recorded inputs into the firmware and compare
/// the setpoints it then computes with the ones the car logged at that point.
pub fn replay(o: &ReplayOptions) -> Result<String, String> {
    let text =
        std::fs::read_to_string(&o.cells).map_err(|e| format!("{}: {e}", o.cells.display()))?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<&str> = lines
        .next()
        .ok_or("empty cells file")?
        .split(',')
        .map(str::trim)
        .collect();
    let col = |name: &str| header.iter().position(|h| *h == name);
    let rpm_col = col("rpm").ok_or("the cells file has no 'rpm' column")?;
    let n_col = col("n");
    let driven: Vec<(usize, &'static str)> = DRIVEN
        .iter()
        .filter_map(|&(c, key)| Some((col(c)?, key)))
        .collect();
    // Each compared setpoint that the cells file carries and the firmware answers for.
    let compared: Vec<(&'static str, usize, usize)> = COMPARED
        .iter()
        .filter_map(|&(c, did)| Some((c, col(c)?, setpoint_of(did)?)))
        .collect();
    if compared.is_empty() {
        return Err("the cells file names none of the compared setpoints".into());
    }

    let mut cells = Vec::new();
    for line in lines {
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        let get = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok());
        let rpm = get(rpm_col).ok_or("a cell has no readable rpm")? as u32;
        let drv = driven
            .iter()
            .filter_map(|&(i, key)| Some((key, get(i)?)))
            .collect();
        let recorded = compared.iter().map(|&(_, i, _)| get(i)).collect();
        let n = n_col.and_then(get).unwrap_or(0.0) as u64;
        cells.push(Cell {
            rpm,
            driven: drv,
            recorded,
            n,
        });
    }
    if cells.is_empty() {
        return Err("the cells file has no rows".into());
    }

    // Run the firmware at every cell, in parallel, as the grid does.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let computed: Vec<Result<Vec<Option<f64>>, String>> = std::thread::scope(|s| {
        let chunks: Vec<_> = cells
            .chunks(cells.len().div_ceil(threads).max(1))
            .map(|chunk| {
                s.spawn(|| {
                    chunk
                        .iter()
                        .map(|c| read_point(&at_cell(&o.ecu, &o.ecu.flash, c)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        chunks
            .into_iter()
            .flat_map(|h| h.join().expect("replay thread"))
            .collect()
    });

    // Per compared setpoint, the absolute error over the cells the ECU answered.
    let mut out = String::new();
    let samples: u64 = cells.iter().map(|c| c.n).sum();
    writeln!(
        out,
        "Replayed {} cells ({samples} samples) through the firmware.\n",
        cells.len()
    )
    .unwrap();
    writeln!(
        out,
        "{:<22}{:>8}{:>10}{:>10}{:>10}   worst at",
        "setpoint", "cells", "mean|e|", "median|e|", "p90|e|"
    )
    .unwrap();
    for (ci, &(name, _, sp)) in compared.iter().enumerate() {
        let s = &SETPOINTS[sp];
        let mut errs: Vec<(f64, &Cell)> = Vec::new();
        for (cell, got) in cells.iter().zip(&computed) {
            if let (Ok(got), Some(rec)) = (got, cell.recorded[ci]) {
                if let Some(c) = got[sp] {
                    errs.push(((c - rec).abs(), cell));
                }
            }
        }
        if errs.is_empty() {
            writeln!(out, "{:<22}{:>8}", format!("{} ({})", name, s.unit), 0).unwrap();
            continue;
        }
        let mut sorted: Vec<f64> = errs.iter().map(|&(e, _)| e).collect();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
        let median = sorted[sorted.len() / 2];
        let p90 = sorted[(sorted.len() * 9 / 10).min(sorted.len() - 1)];
        let worst = errs.iter().max_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        writeln!(
            out,
            "{:<22}{:>8}{:>10.2}{:>10.2}{:>10.2}   {:.1} at {} rpm",
            format!("{} ({})", name, s.unit),
            errs.len(),
            mean,
            median,
            p90,
            worst.0,
            worst.1.rpm
        )
        .unwrap();
    }
    writeln!(
        out,
        "\n|e| is |computed - recorded|. Fuel per stroke is 0 without injection, so the\n\
         setpoints that depend on it (and anything downstream) will not match here."
    )
    .unwrap();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::span;

    fn grid(vs: &[&[Option<f64>]]) -> Vec<Vec<Option<f64>>> {
        vs.iter().map(|row| row.to_vec()).collect()
    }

    #[test]
    fn span_describes_a_grid() {
        // Empty, so nobody reads a range off a grid the ECU never answered.
        assert_eq!(span(&grid(&[&[None, None], &[None, None]])), "no values");
        // Flat at zero: the trap that made a comparison of two of these look meaningful.
        assert_eq!(
            span(&grid(&[&[Some(0.0), Some(0.0)], &[Some(0.0), None]])),
            "all zero"
        );
        // Flat, but not zero (an idle value held across the grid).
        assert_eq!(
            span(&grid(&[&[Some(32.7), Some(32.7)]])),
            "constant at 32.70"
        );
        // A real span.
        assert_eq!(
            span(&grid(&[&[Some(-10.0), Some(40.16)], &[None, Some(0.0)]])),
            "range -10.00..40.16"
        );
    }
}
