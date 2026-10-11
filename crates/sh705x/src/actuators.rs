//! `sh705x actuators`: close the SCV current loop and the EGR position loop against the
//! firmware and trace them, immobiliser or no immobiliser.
//!
//! These two loops run whatever the fuel cut does: the firmware regulates the suction
//! control valve's current on ATU-II output 6A, reading it back on AN1, and the EGR
//! valve's position through an H-bridge on 7D (opening) and 7C (closing), reading it back
//! on AN10 (resultados fase 9). Here the firmware's duties drive the plants of
//! [`engine::actuators`] every millisecond from power-on ([`crate::rig`]), whose output is
//! fed back into those inputs, so the firmware's own regulators close onto their
//! setpoints. The engine is held at one speed; the VGT command (7A) is traced, not
//! closed. The trace is the dynamic data the injection chain cannot give while the
//! immobiliser holds it shut.

use crate::rig::Rig;
use crate::{EcuOptions, f32_at};
use std::fmt::Write as _;

/// Options of `sh705x actuators`.
#[derive(Debug, PartialEq, Eq)]
pub struct ActuatorsOptions {
    /// The ECU and the scenario (speed, sensors).
    pub ecu: EcuOptions,
    /// ECU time to run, milliseconds.
    pub millis: u64,
    /// Sample and feedback interval, milliseconds.
    pub step: u64,
}

/// ATU-II outputs, as (channel 0/1 = 6/7, timer 0..3 = A..D). The SCV is on 6A: the
/// firmware writes its command (0x323D, `FFFEF9C0`, % on-time) there, with a period it
/// varies (`0xC4A4`) [MED].
pub(crate) const SCV_OUT: (usize, usize) = (0, 0); // 6A
/// The VGT's solenoid: 7A carries the vane command that 0x2041 reports (`FFFEFDDC`)
/// [MED].
pub(crate) const VGT_OUT: (usize, usize) = (1, 0); // 7A
/// The EGR's H-bridge: the half that opens it, and the half that closes it. The firmware
/// puts a positive command on 7D and a negative one on 7C (`0xB76C`) [MED].
pub(crate) const EGR_OPEN: (usize, usize) = (1, 3); // 7D
pub(crate) const EGR_CLOSE: (usize, usize) = (1, 2); // 7C

/// The plants' step, milliseconds.
const PLANT_MS: u64 = 1;

/// Where the firmware keeps the SCV current it asks for, mA: the setpoint its current
/// loop on 6A closes onto (found as the float the measured current settles at, beside
/// the command it drives) [MED].
pub const SCV_DESIRED_AT: u32 = 0xFFFE_F9DC;

/// Where the firmware keeps the EGR position it asks for, % (what 0x002C, desired EGR
/// position, reports) [MED].
pub const EGR_DESIRED_AT: u32 = 0xFFFF_1370;

/// One sample of the trace.
struct Sample {
    t: f64,
    scv_duty: Option<f64>,
    scv_ma: f64,
    scv_desired: f64,
    vgt: Option<f64>,
    egr_drive: f64,
    egr_pos: f64,
    egr_desired: f64,
    maf: f64,
}

/// Runs `sh705x actuators`.
pub fn actuators(o: &ActuatorsOptions) -> Result<String, String> {
    let mut rig = Rig::new(&o.ecu, None, PLANT_MS * 1000)?;
    // The plants step every millisecond, as fast as the firmware's own loops, whatever
    // the trace interval: a slower feedback would be a plant the firmware never sees.
    let every = (o.step / PLANT_MS).max(1);
    let samples = o.millis / PLANT_MS;

    let mut out = String::new();
    writeln!(
        out,
        "SCV current loop (6A -> AN1), EGR position loop (7D/7C -> AN10) and VGT command \
         (7A), {} ms, every {} ms",
        o.millis, o.step
    )
    .unwrap();
    writeln!(
        out,
        "{:>7} {:>8} {:>8} {:>8} {:>9} {:>7} {:>7} {:>7} {:>7}",
        "t/ms", "scv duty", "scv/mA", "asked/mA", "egr drive", "egr/%", "asked/%", "maf/g/s", "vgt"
    )
    .unwrap();

    let mut trace: Vec<Sample> = Vec::new();
    for s in 0..samples {
        let st = rig.step();
        if (s + 1) % every != 0 {
            continue;
        }
        trace.push(Sample {
            t: ((s + 1) * PLANT_MS) as f64,
            scv_duty: st.scv_duty,
            scv_ma: st.scv_ma,
            scv_desired: f32_at(&rig.m, SCV_DESIRED_AT),
            vgt: st.vgt_duty,
            egr_drive: st.egr_drive,
            egr_pos: st.egr,
            egr_desired: f32_at(&rig.m, EGR_DESIRED_AT),
            maf: st.maf,
        });
    }

    // Show at most ~30 rows, thinning evenly, but always the last.
    let keep = (trace.len() / 30).max(1);
    for (i, r) in trace.iter().enumerate() {
        if i % keep == 0 || i + 1 == trace.len() {
            let d = |x: Option<f64>| x.map_or("-".into(), |v| format!("{v:.3}"));
            writeln!(
                out,
                "{:>7.0} {:>8} {:>8.0} {:>8.0} {:>9.3} {:>7.2} {:>7.2} {:>7.2} {:>7}",
                r.t,
                d(r.scv_duty),
                r.scv_ma,
                r.scv_desired,
                r.egr_drive,
                r.egr_pos,
                r.egr_desired,
                r.maf,
                d(r.vgt)
            )
            .unwrap();
        }
    }

    // A loop has settled if its last quarter of samples barely moves.
    let tail = &trace[trace.len() - trace.len().div_ceil(4)..];
    let settle = |pick: fn(&Sample) -> f64| {
        let (lo, hi) = tail
            .iter()
            .map(pick)
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
        (tail.last().map(pick).unwrap_or(0.0), hi - lo)
    };
    let mean = |pick: fn(&Sample) -> f64| tail.iter().map(pick).sum::<f64>() / tail.len() as f64;
    let (scv_end, scv_span) = settle(|r| r.scv_ma);
    let (egr_end, egr_span) = settle(|r| r.egr_pos);
    writeln!(out).unwrap();
    writeln!(
        out,
        "SCV settled at {scv_end:.0} mA (last quarter spans {scv_span:.0} mA), asked for \
         {:.0} mA; EGR at {egr_end:.1} % (spans {egr_span:.1} %), asked for {:.1} %, held with a mean \
         drive of {:.3}.",
        mean(|r| r.scv_desired),
        mean(|r| r.egr_desired),
        mean(|r| r.egr_drive),
    )
    .unwrap();
    Ok(out)
}
