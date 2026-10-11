//! The plants aroun        self.scv.step(scv_duty.unwrap_or(0.0), self.dt); the ECU, stepped together from power-on: the SCV's current, the EGR
//! valve and the fresh air it leaves, and, if asked, the crankshaft. What the firmware
//! drives goes into them and what they give comes back to its inputs every step, so its
//! own loops close through them. `sh705x actuators` holds the engine at one speed;
//! `sh705x plant` turns it on the firmware's torque.
//!
//! The plants run from the first instruction: the firmware tests some actuators at
//! power-on (it sweeps the EGR valve), and a plant that only starts later fails those
//! tests (resultados fase 9).

use crate::actuators::{EGR_CLOSE, EGR_OPEN, SCV_OUT, VGT_OUT};
use crate::{EcuOptions, boot, f32_at, sensors};
use engine::actuators::{Egr, Solenoid};
use engine::crank::Crank;
use engine::sensors::{Input, MAF_KEY, input, maf_hz};
use engine::turbo::Turbo;
use sh7059::Machine;

/// Where the firmware keeps its net torque, N·m (what 0x1A2D reports) [MED]: see
/// [`crate::plant::TORQUE_AT`].
use crate::plant::{QUANTITY_AT, TORQUE_AT};

/// What one step gave.
#[derive(Clone, Copy, Debug)]
pub(crate) struct State {
    /// Engine speed, rpm.
    pub rpm: f64,
    /// SCV duty the firmware drives, if its timer runs.
    pub scv_duty: Option<f64>,
    /// VGT duty, if its timer runs.
    pub vgt_duty: Option<f64>,
    /// SCV current, mA.
    pub scv_ma: f64,
    /// EGR net drive, opening minus closing, -1..1.
    pub egr_drive: f64,
    /// EGR position, % open.
    pub egr: f64,
    /// Fresh air the MAF reads, g/s; 0 with the engine stopped.
    pub maf: f64,
    /// Manifold pressure, kPa: the sensors' resting value plus the turbocharger's boost.
    pub boost: f64,
    /// The firmware's net torque, N·m.
    pub torque: f64,
}

/// The plants and the inputs they feed.
pub(crate) struct Rig {
    /// The machine, running.
    pub m: Machine,
    /// The crankshaft, if the engine turns on the firmware's torque; otherwise the speed is
    /// held where [`EcuOptions::rpm`] puts it.
    pub crank: Option<Crank>,
    scv: Solenoid,
    egr: Egr,
    turbo: Turbo,
    scv_in: &'static Input,
    egr_in: &'static Input,
    boost_in: &'static Input,
    /// Manifold pressure with no boost, kPa (the sensors' value).
    boost: f64,
    intake: f64,
    rpm: f64,
    /// Step, seconds.
    pub dt: f64,
    /// The crankshaft is held at its speed (while the firmware boots).
    held: bool,
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
        let mut rig = Self {
            m: boot(&cold)?,
            rpm: crank.as_ref().map_or(o.rpm.unwrap_or(0) as f64, |c| c.rpm),
            crank,
            scv: Solenoid::scv(),
            egr: Egr::new(),
            turbo: Turbo::new(),
            scv_in: input("scv-feedback").expect("scv-feedback curve"),
            egr_in: input("egr").expect("egr curve"),
            boost_in: input("boost").expect("boost curve"),
            boost: get("boost").unwrap_or(98.0),
            intake: get("intake2").or(get("intake")).unwrap_or(26.0),
            dt: step_us as f64 / 1e6,
            held: true,
        };
        let steps = o.boot / (step_us * bridge::link::CYCLES_PER_US).max(1);
        // The engine turns at its starting speed while the firmware boots: its torque only
        // means something once it runs its idle governor.
        for _ in 0..steps {
            rig.step();
        }
        rig.held = false;
        Ok(rig)
    }

    /// Runs one step of ECU time, then steps the plants on what the firmware drives and
    /// feeds them back.
    pub fn step(&mut self) -> State {
        let m = &mut self.m;
        m.run((self.dt * 1e6) as u64 * bridge::link::CYCLES_PER_US);
        m.chip.hcan.ch[0].outbox.clear();
        let torque = f32_at(m, TORQUE_AT);
        if let Some(c) = self.crank.as_mut().filter(|_| !self.held) {
            let was = self.rpm;
            self.rpm = c.step(torque, self.dt);
            if self.rpm != was {
                m.chip.stimulus_set("rpm", self.rpm);
            }
        }
        let duty = |(c, k): (usize, usize)| m.chip.pwm_duty(c, k);
        let scv_duty = duty(SCV_OUT);
        let vgt_duty = duty(VGT_OUT);
        let egr_drive = duty(EGR_OPEN).unwrap_or(0.0) - duty(EGR_CLOSE).unwrap_or(0.0);
        self.scv.step(scv_duty.unwrap_or(0.0), self.dt);
        let egr = self.egr.step(egr_drive, self.dt);
        m.chip.adc.input[self.scv_in.channel] = self.scv_in.code(self.scv.milliamps());
        m.chip.adc.input[self.egr_in.channel] = self.egr_in.code(egr);
        // The turbine on the fuel the firmware asks for, with its vanes where 7A puts them.
        let q = f32_at(m, QUANTITY_AT);
        let vgt = vgt_duty.unwrap_or(0.0) * 100.0;
        let boost = self.boost + self.turbo.step(self.rpm, q, vgt, self.dt);
        m.chip.adc.input[self.boost_in.channel] = self.boost_in.code(boost);
        let maf = if self.rpm > 0.0 {
            let g = engine::air::fresh_air_g_per_s(self.rpm, boost, self.intake, egr);
            m.chip.stimulus_set(MAF_KEY, maf_hz(g));
            g
        } else {
            0.0
        };
        State {
            rpm: self.rpm,
            scv_duty,
            vgt_duty,
            scv_ma: self.scv.milliamps(),
            egr_drive,
            egr,
            maf,
            boost,
            torque,
        }
    }
}
