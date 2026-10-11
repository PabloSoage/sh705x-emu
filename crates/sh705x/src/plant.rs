//! `sh705x plant`: run the engine on the firmware's own torque demand, immobiliser or
//! no immobiliser.
//!
//! With the immobiliser shut the firmware arms no injector, but it still computes how
//! much fuel it wants and the torque that fuel would give (resultados fase 8). Here a
//! crankshaft ([`engine::crank`]) turns under that torque, the crank wheel follows its
//! speed tooth by tooth, the MAF reads what the engine draws at that speed, and the SCV
//! and EGR plants close as in `sh705x actuators`. So the firmware's idle governor and
//! pedal map close their loop through a simulated engine: nothing in the firmware is
//! changed, and nothing here would make a real engine run.

use crate::rig::Rig;
use crate::{EcuOptions, f32_at};
use engine::crank::Crank;
use sh7059::Machine;
use std::fmt::Write as _;

/// Options of `sh705x plant`.
#[derive(Debug, PartialEq)]
pub struct PlantOptions {
    /// The ECU and the scenario (starting speed, sensors, pedal).
    pub ecu: EcuOptions,
    /// ECU time to run, milliseconds.
    pub millis: u64,
    /// Plant step, microseconds.
    pub step_us: u64,
    /// External load on the crank, N·m: by default [`IDLE_LOAD`].
    pub load: f64,
}

/// The load on a warm idling Astra beyond the friction the firmware models, N·m: the
/// torque its ECU reports then, which at a steady speed equals that load (median of
/// scanmatik-bench routes 67, 69 and 72, standing at 650-900 rpm with coolant at 70 °C
/// or more: 12.0 N·m, 9090 samples) [DER].
pub const IDLE_LOAD: f64 = 12.0;

/// Where the firmware keeps its net torque (what 0x1A2D reports): a float in RAM, found
/// by searching the RAM for the diagnostic value and following it over speeds [MED]. In
/// the Astra's software (calibration NCLSW420AX36); another software may keep it
/// elsewhere, which the check against 0x1A2D at the start catches.
pub const TORQUE_AT: u32 = 0xFFFE_FC4C;
/// Where it keeps the fuel quantity it asks for, mm³ a stroke: the float that the
/// rail-pressure maps take as their quantity input, and that follows the idle governor
/// over speeds (34 at 600 rpm, 12.5 at 780, -10 from 800 with the engine held) [MED].
pub const QUANTITY_AT: u32 = 0xFFFE_F8F4;
/// Where it keeps the idle speed it aims at, rpm: the largest of the requests it weighs
/// (coolant, limp modes...), found as the float that follows the idle when the engine
/// is held at one speed [MED].
pub const IDLE_TARGET_AT: u32 = 0xFFFE_FB60;

/// The torque the firmware reports on diagnostics, N·m (0x1A2D, signed, 0.25 a bit).
fn diagnostic_torque(m: &mut Machine) -> Option<f64> {
    let mut link = bridge::link::VirtualLink::new(m, 0);
    let mut elm = bridge::elm::Elm327::new();
    elm.echo = false;
    let b: Vec<u8> = elm
        .line("221A2D", &mut link)
        .split_whitespace()
        .filter_map(|h| u8::from_str_radix(h, 16).ok())
        .collect();
    let at = b.windows(3).position(|w| w == [0x62, 0x1A, 0x2D])? + 3;
    let raw = i16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]);
    Some(raw as f64 * 0.25)
}

/// One sample of the trace.
struct Sample {
    t: f64,
    rpm: f64,
    target: f64,
    torque: f64,
    quantity: f64,
    maf: f64,
    egr: f64,
}

/// Runs `sh705x plant`.
pub fn plant(o: &PlantOptions) -> Result<String, String> {
    let mut crank = Crank::new(o.ecu.rpm.unwrap_or(750) as f64);
    crank.load = o.load;
    let mut rig = Rig::new(&o.ecu, Some(crank), o.step_us)?;

    // The RAM must hold what the diagnostics say, or this is another software.
    let said = diagnostic_torque(&mut rig.m).ok_or("no answer to 0x1A2D")?;
    let held = f32_at(&rig.m, TORQUE_AT);
    if (said - held).abs() > 0.5 {
        return Err(format!(
            "the torque is not at {TORQUE_AT:08X} in this image ({held} there, {said} on \
             diagnostics): another software"
        ));
    }

    let samples = o.millis * 1000 / o.step_us.max(1);
    let mut trace: Vec<Sample> = Vec::new();
    for k in 0..samples {
        let st = rig.step();
        trace.push(Sample {
            t: (k + 1) as f64 * rig.dt * 1000.0,
            rpm: st.rpm,
            target: f32_at(&rig.m, IDLE_TARGET_AT),
            torque: st.torque,
            quantity: f32_at(&rig.m, QUANTITY_AT),
            maf: st.maf,
            egr: st.egr,
        });
    }

    let mut out = String::new();
    writeln!(
        out,
        "Engine on the firmware's torque: {} ms, every {} µs, external load {} N·m",
        o.millis, o.step_us, o.load
    )
    .unwrap();
    writeln!(
        out,
        "{:>8} {:>8} {:>8} {:>10} {:>12} {:>8} {:>7}",
        "t/ms", "rpm", "target", "torque/Nm", "fuel/mm3", "maf/g/s", "egr/%"
    )
    .unwrap();
    // Show at most ~40 rows, thinning evenly, but always the last.
    let keep = (trace.len() / 40).max(1);
    for (i, r) in trace.iter().enumerate() {
        if i % keep == 0 || i + 1 == trace.len() {
            writeln!(
                out,
                "{:>8.0} {:>8.1} {:>8.0} {:>10.2} {:>12.2} {:>8.2} {:>7.1}",
                r.t, r.rpm, r.target, r.torque, r.quantity, r.maf, r.egr
            )
            .unwrap();
        }
    }
    // Settling: the speed over the last quarter.
    let tail = &trace[trace.len() - trace.len().div_ceil(4)..];
    let (lo, hi) = tail
        .iter()
        .map(|r| r.rpm)
        .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
    let mean = |f: fn(&Sample) -> f64| tail.iter().map(f).sum::<f64>() / tail.len() as f64;
    writeln!(out).unwrap();
    writeln!(
        out,
        "Last quarter: {:.0} rpm on average (from {lo:.0} to {hi:.0}), aiming at an idle of \
         {:.0}; {:.1} N·m, {:.2} mm3 a stroke asked for; EGR {:.1} % open, {:.2} g/s of \
         fresh air.",
        mean(|r| r.rpm),
        mean(|r| r.target),
        mean(|r| r.torque),
        mean(|r| r.quantity),
        mean(|r| r.egr),
        mean(|r| r.maf),
    )
    .unwrap();
    Ok(out)
}
