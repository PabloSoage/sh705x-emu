//! The engine's history, for the engine view: what the firmware asks for and what the
//! plants give back, sampled in ECU time, and the one-shot pulses of the ATU-II's channel 8
//! (where the injectors are). Kept apart from the snapshot, so a frame does not copy it.

use engine::plants::{State, f32_at};
use sh7059::Machine;
use std::collections::VecDeque;

/// One trace: name, unit.
pub struct Trace {
    /// What it is.
    pub name: &'static str,
    /// Its unit.
    pub unit: &'static str,
}

/// The traces, in [`Sample::v`] order.
pub const TRACES: [Trace; 15] = [
    Trace {
        name: "engine speed",
        unit: "rpm",
    },
    Trace {
        name: "EGR asked",
        unit: "%",
    },
    Trace {
        name: "EGR position",
        unit: "%",
    },
    Trace {
        name: "EGR drive (open +, close −)",
        unit: "%",
    },
    Trace {
        name: "boost asked",
        unit: "kPa",
    },
    Trace {
        name: "boost",
        unit: "kPa",
    },
    Trace {
        name: "VGT command",
        unit: "%",
    },
    Trace {
        name: "SCV current asked",
        unit: "mA",
    },
    Trace {
        name: "SCV current",
        unit: "mA",
    },
    Trace {
        name: "SCV command",
        unit: "%",
    },
    Trace {
        name: "rail pressure asked",
        unit: "MPa",
    },
    Trace {
        name: "rail pressure (sensor)",
        unit: "MPa",
    },
    Trace {
        name: "fresh air",
        unit: "g/s",
    },
    Trace {
        name: "fuel asked",
        unit: "mm³",
    },
    Trace {
        name: "torque asked",
        unit: "N·m",
    },
];

/// Where the firmware keeps its setpoints in RAM, as floats (NCLSW420AX36) [MED].
const EGR_ASKED_AT: u32 = 0xFFFF_1370;
const BOOST_ASKED_AT: u32 = 0xFFFE_FE84;
const SCV_ASKED_AT: u32 = 0xFFFE_F9DC;
const RAIL_ASKED_AT: u32 = 0xFFFE_F924;

/// One moment.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// ECU time, µs.
    pub us: f64,
    /// Crankshaft angle over 720°, NaN with the engine stopped.
    pub crank: f32,
    /// The traces; NaN where there is nothing to show.
    pub v: [f32; TRACES.len()],
}

/// The history.
#[derive(Debug, Default)]
pub struct Scope {
    /// Samples, oldest first.
    pub samples: VecDeque<Sample>,
    /// Channel 8 one-shot pulses: output (0 = TO8A .. 15 = TO8P), start and end, µs.
    pub pulses: VecDeque<(u8, f64, f64)>,
    /// Cycle of the last sample.
    last: Option<u64>,
    /// End of the last pulse taken, cycles.
    last_pulse: u64,
}

/// Samples kept: about 30 s of ECU time at the usual pace.
const KEPT: usize = 60_000;
/// Pulses kept.
const PULSES_KEPT: usize = 20_000;
/// Running, a sample at most this often, cycles: 100 µs.
const EVERY: u64 = 8_000;
/// CPU cycles per µs.
const PER_US: f64 = 80.0;

impl Scope {
    /// Takes a sample of `m` if enough ECU time has gone by (or any, with `stepping`), with
    /// the plants' last step if they run, else what the engine is set to.
    pub fn take(
        &mut self,
        m: &Machine,
        plant: Option<&State>,
        engine: &crate::emu::Engine,
        stepping: bool,
    ) {
        let now = m.cpu.cycles;
        let due = match self.last {
            None => true,
            Some(l) => now > l && (stepping || now - l >= EVERY),
        };
        // The pulses that ended since the last look.
        for &(k, start, end) in &m.chip.atu.pulses {
            if end > self.last_pulse {
                self.pulses
                    .push_back((k, start as f64 / PER_US, end as f64 / PER_US));
                self.last_pulse = end;
            }
        }
        while self.pulses.len() > PULSES_KEPT {
            self.pulses.pop_front();
        }
        if !due {
            return;
        }
        self.last = Some(now);
        let sensor = |k: &str| {
            engine
                .sensors
                .values
                .iter()
                .find(|(x, _)| *x == k)
                .map_or(f64::NAN, |&(_, v)| v)
        };
        let ram = |a| f32_at(m, a);
        let duty = |ch, k| m.chip.pwm_duty(ch, k).map_or(f64::NAN, |d| d * 100.0);
        let egr_drive = {
            let w = engine::plants::ASTRA;
            let open = m.chip.pwm_duty(w.egr_open.0, w.egr_open.1);
            let close = m.chip.pwm_duty(w.egr_close.0, w.egr_close.1);
            (open.unwrap_or(0.0) - close.unwrap_or(0.0)) * 100.0
        };
        let v = [
            plant.map_or(engine.rpm, |p| p.rpm),
            ram(EGR_ASKED_AT),
            plant.map_or(sensor("egr"), |p| p.egr),
            egr_drive,
            ram(BOOST_ASKED_AT),
            plant.map_or(sensor("boost"), |p| p.boost),
            duty(1, 0),
            ram(SCV_ASKED_AT),
            plant.map_or(f64::NAN, |p| p.scv_ma),
            duty(0, 0),
            ram(RAIL_ASKED_AT),
            sensor("rail"),
            plant.map_or(sensor("maf"), |p| p.maf),
            ram(engine::plants::ASTRA.quantity_at),
            ram(engine::plants::ASTRA.torque_at),
        ];
        self.samples.push_back(Sample {
            us: now as f64 / PER_US,
            crank: m.chip.crank_angle(now).map_or(f32::NAN, |a| a as f32),
            v: v.map(|x| x as f32),
        });
        while self.samples.len() > KEPT {
            self.samples.pop_front();
        }
    }
}
