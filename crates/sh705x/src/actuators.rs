//! `sh705x actuators`: close the SCV current loop and the EGR position loop against the
//! firmware and trace them, immobiliser or no immobiliser.
//!
//! These two loops run whatever the fuel cut does: the firmware regulates the suction
//! control valve's current on ATU-II output 7A, reading it back on AN1, and the EGR
//! valve's position on 7C, reading it back on AN10 (resultados fase 5 §6). Here the
//! firmware's duties drive the plants of [`engine::actuators`], whose output is fed back
//! into those inputs every sample, so the firmware's own regulators close onto their
//! setpoints. The trace is the dynamic data the injection chain cannot give while the
//! immobiliser holds it shut.

use crate::{EcuOptions, boot};
use engine::actuators::{Egr, Solenoid};
use engine::sensors::input;
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

/// ATU-II output of each loop, as (channel 0/1 = 6/7, timer 0..3 = A..D).
const SCV_OUT: (usize, usize) = (1, 0); // 7A
const EGR_OUT: (usize, usize) = (1, 2); // 7C

/// One sample of the trace.
struct Sample {
    t: f64,
    scv_duty: Option<f64>,
    scv_ma: f64,
    egr_duty: Option<f64>,
    egr_pos: f64,
}

/// Runs `sh705x actuators`.
pub fn actuators(o: &ActuatorsOptions) -> Result<String, String> {
    let scv_in = input("scv-feedback").expect("scv-feedback curve");
    let egr_in = input("egr").expect("egr curve");
    let mut m = boot(&o.ecu)?;

    let mut scv = Solenoid::scv();
    let mut egr = Egr::new();
    let dt = o.step as f64 / 1000.0;
    let slice = o.step * bridge::link::CYCLES_PER_US * 1000;
    let samples = o.millis / o.step.max(1);

    let mut out = String::new();
    writeln!(
        out,
        "SCV current loop (7A -> AN1) and EGR position loop (7C -> AN10), {} ms, every {} ms",
        o.millis, o.step
    )
    .unwrap();
    writeln!(
        out,
        "{:>7} {:>8} {:>10} {:>8} {:>9}",
        "t/ms", "scv duty", "scv/mA", "egr duty", "egr/%"
    )
    .unwrap();

    let mut trace: Vec<Sample> = Vec::new();
    for s in 0..samples {
        m.run(slice);
        m.chip.hcan.ch[0].outbox.clear();
        // Read the duties the firmware is driving, step the plants, feed them back.
        let scv_duty = m.chip.pwm_duty(SCV_OUT.0, SCV_OUT.1);
        let egr_duty = m.chip.pwm_duty(EGR_OUT.0, EGR_OUT.1);
        scv.step(scv_duty.unwrap_or(0.0), dt);
        let egr_pos = egr.step(egr_duty.unwrap_or(0.0), dt);
        m.chip.adc.input[scv_in.channel] = scv_in.code(scv.milliamps());
        m.chip.adc.input[egr_in.channel] = egr_in.code(egr_pos);
        trace.push(Sample {
            t: (s + 1) as f64 * o.step as f64,
            scv_duty,
            scv_ma: scv.milliamps(),
            egr_duty,
            egr_pos,
        });
    }

    // Show at most ~30 rows, thinning evenly, but always the last.
    let keep = (trace.len() / 30).max(1);
    for (i, r) in trace.iter().enumerate() {
        if i % keep == 0 || i + 1 == trace.len() {
            let d = |x: Option<f64>| x.map_or("-".into(), |v| format!("{v:.3}"));
            writeln!(
                out,
                "{:>7.0} {:>8} {:>10.1} {:>8} {:>9.2}",
                r.t,
                d(r.scv_duty),
                r.scv_ma,
                d(r.egr_duty),
                r.egr_pos
            )
            .unwrap();
        }
    }

    // A loop has settled if its last quarter of samples barely moves.
    let settle = |pick: fn(&Sample) -> f64| {
        let tail = &trace[trace.len() - trace.len().div_ceil(4)..];
        let (lo, hi) = tail
            .iter()
            .map(pick)
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
        (tail.last().map(pick).unwrap_or(0.0), hi - lo)
    };
    let (scv_end, scv_span) = settle(|r| r.scv_ma);
    let (egr_end, egr_span) = settle(|r| r.egr_pos);
    writeln!(out).unwrap();
    writeln!(
        out,
        "SCV settled at {scv_end:.0} mA (last quarter spans {scv_span:.0} mA); \
         EGR at {egr_end:.1} % (spans {egr_span:.1} %).",
    )
    .unwrap();
    Ok(out)
}
