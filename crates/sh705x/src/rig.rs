//! The ECU powered on inside its plants ([`engine::plants`]), stepped together from the
//! first instruction. `sh705x actuators` and `maps` hold the engine at one speed; `sh705x
//! plant` turns it on the firmware's torque.

use crate::{EcuOptions, boot, sensors};
use engine::crank::Crank;
pub(crate) use engine::plants::State;
use engine::plants::{ASTRA, Plants};
use sh7059::Machine;

/// The machine and its plants.
pub(crate) struct Rig {
    /// The machine, running.
    pub m: Machine,
    /// The plants around it.
    pub plants: Plants,
    /// Step, seconds.
    pub dt: f64,
}

impl Rig {
    /// Powers the ECU on with the plants around it, steps of `step_us` microseconds, and
    /// runs [`EcuOptions::boot`] instructions' worth of steps before handing it back,
    /// with the crankshaft held at its starting speed until then.
    pub fn new(o: &EcuOptions, crank: Option<Crank>, step_us: u64) -> Result<Self, String> {
        let s = sensors(o)?;
        let get = |k: &str| s.values.iter().find(|(x, _)| *x == k).map(|&(_, v)| v);
        let mut cold = o.clone();
        cold.boot = 0;
        let mut plants = Plants::new(
            ASTRA,
            o.rpm.unwrap_or(0) as f64,
            crank,
            get("boost").unwrap_or(98.0),
            get("intake2").or(get("intake")).unwrap_or(26.0),
        );
        plants.held = true;
        let mut rig = Self {
            m: boot(&cold)?,
            plants,
            dt: step_us as f64 / 1e6,
        };
        let steps = o.boot / (step_us * bridge::link::CYCLES_PER_US).max(1);
        for _ in 0..steps {
            rig.step();
        }
        rig.plants.held = false;
        Ok(rig)
    }

    /// Runs one step of ECU time, then steps the plants.
    pub fn step(&mut self) -> State {
        self.m
            .run((self.dt * 1e6) as u64 * bridge::link::CYCLES_PER_US);
        self.m.chip.hcan.ch[0].outbox.clear();
        self.plants.step(&mut self.m, self.dt)
    }
}
