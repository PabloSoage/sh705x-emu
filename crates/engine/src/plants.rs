//! The plants around the ECU, stepped together: the SCV's current, the EGR valve and the
//! fresh air it leaves, the turbocharger, and, if asked, the crankshaft on the firmware's
//! torque. What the firmware drives goes into them and what they give comes back to its
//! inputs, so its own loops close through them.
//!
//! [`Plants::step`] follows a stretch of ECU time the caller has just run: the command
//! line steps them every millisecond from power-on (the firmware tests some actuators at
//! power-on, and a plant that starts later fails those tests: resultados fase 9), and the
//! GUI after each slice it runs.

use crate::actuators::{Egr, Solenoid};
use crate::bus::Road;
use crate::crank::Crank;
use crate::rail::Rail;
use crate::sensors::{Input, MAF_KEY, input, maf_hz};
use crate::software::Software;
use crate::turbo::Turbo;
use crate::vehicle::Vehicle;
use sh7059::Machine;

/// Where the plants meet one ECU: its outputs, as ATU-II (channel 0/1 = 6/7, timer
/// 0..3 = A..D), and the floats in RAM they read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wiring {
    /// The SCV's PWM.
    pub scv: (usize, usize),
    /// The VGT's solenoid.
    pub vgt: (usize, usize),
    /// The half of the EGR's H-bridge that opens it.
    pub egr_open: (usize, usize),
    /// The half that closes it.
    pub egr_close: (usize, usize),
    /// The firmware's net torque, N·m (what 0x1A2D reports), where its release is known.
    pub torque_at: Option<u32>,
    /// The fuel per stroke it asks for, mm³, where its release is known.
    pub quantity_at: Option<u32>,
    /// The rail pressure it asks for, MPa (what 0x308A reports), where its release is known.
    pub rail_asked_at: Option<u32>,
}

/// The outputs of the Astra J's board (A17DTJ), found in its firmware NCLSW420AX36 [MED],
/// with no release's variables (see [`Wiring::of`]):
/// - the SCV on 6A: the firmware writes its command (0x323D, `FFFEF9C0`, % on-time) there,
///   with a period it varies (`0xC4A4`);
/// - the VGT on 7A, which carries the vane command 0x2041 reports (`FFFEFDDC`);
/// - the EGR's H-bridge: a positive command on 7D, a negative one on 7C (`0xB76C`).
pub const ASTRA: Wiring = Wiring {
    scv: (0, 0),
    vgt: (1, 0),
    egr_open: (1, 3),
    egr_close: (1, 2),
    torque_at: None,
    quantity_at: None,
    rail_asked_at: None,
};

impl Wiring {
    /// The Astra's board running `software`; without its variables if it is not a release
    /// known, so that the plants never read another release's.
    pub fn of(software: Option<&Software>) -> Self {
        Self {
            torque_at: software.map(|s| s.ram.torque),
            quantity_at: software.map(|s| s.ram.quantity),
            rail_asked_at: software.map(|s| s.ram.rail_asked),
            ..ASTRA
        }
    }
}

/// A big-endian float in the chip's memory.
pub fn f32_at(m: &Machine, addr: u32) -> f64 {
    let bits = (0..4).fold(0u32, |v, k| v << 8 | m.chip.peek(addr + k) as u32);
    f32::from_bits(bits) as f64
}

/// What one step gave.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
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
    /// Manifold pressure, kPa: the resting value plus the turbocharger's boost.
    pub boost: f64,
    /// The firmware's net torque, N·m; NaN where its variable is not known.
    pub torque: f64,
    /// The fuel per stroke it asks for, mm³; NaN where its variable is not known.
    pub quantity: f64,
    /// Rail pressure, MPa; NaN where the firmware's setpoint is not known.
    pub rail: f64,
    /// Road speed, km/h (0.0 in neutral or without a vehicle).
    pub kmh: f64,
}

/// The plants and the inputs they feed.
#[derive(Clone, Debug)]
pub struct Plants {
    /// Where they meet the ECU.
    pub wiring: Wiring,
    /// The crankshaft, if the engine turns on the firmware's torque; otherwise the speed is
    /// held at [`Plants::rpm`].
    pub crank: Option<Crank>,
    /// Optional vehicle + gearbox coupled to the crankshaft.
    pub vehicle: Option<Vehicle>,
    /// Optional road speed updated on the CAN bus when `vehicle` is set.
    pub road: Option<Road>,
    /// The crankshaft is held at its speed for now (while the firmware boots: its torque
    /// only means something once it runs its idle governor).
    pub held: bool,
    /// Engine speed, rpm.
    pub rpm: f64,
    /// Manifold pressure with no boost, kPa.
    pub rest_kpa: f64,
    /// Manifold air temperature, °C.
    pub intake_c: f64,
    /// The SCV coil.
    pub scv: Solenoid,
    /// The EGR valve.
    pub egr: Egr,
    /// The turbocharger.
    pub turbo: Turbo,
    /// The high-pressure fuel rail.
    pub rail: Rail,
    scv_in: &'static Input,
    egr_in: &'static Input,
    boost_in: &'static Input,
    rail_in: &'static Input,
}

impl Plants {
    /// The plants of `wiring`, the engine at `rpm` (turning on the firmware's torque if
    /// `crank` is given and the torque is known, from `rpm`), the manifold at `rest_kpa` with no boost and
    /// `intake_c`.
    pub fn new(
        wiring: Wiring,
        rpm: f64,
        crank: Option<Crank>,
        rest_kpa: f64,
        intake_c: f64,
    ) -> Self {
        let crank = crank.filter(|_| wiring.torque_at.is_some());
        let rpm = crank.as_ref().map_or(rpm, |c| c.rpm);
        let mut rail = Rail::astra();
        if rpm > 0.0 {
            rail.p = 23.7;
        }
        Self {
            wiring,
            rpm,
            crank,
            vehicle: None,
            road: None,
            held: false,
            rest_kpa,
            intake_c,
            scv: Solenoid::scv(),
            egr: Egr::new(),
            turbo: Turbo::new(),
            rail,
            scv_in: input("scv-feedback").expect("scv-feedback curve"),
            egr_in: input("egr").expect("egr curve"),
            boost_in: input("boost").expect("boost curve"),
            rail_in: input("rail").expect("rail curve"),
        }
    }

    /// Steps the plants over the `dt` seconds of ECU time just run, on what the firmware
    /// drives now, and feeds them back to its inputs.
    pub fn step(&mut self, m: &mut Machine, dt: f64) -> State {
        let w = self.wiring;
        let read = |m: &Machine, at: Option<u32>| at.map_or(f64::NAN, |a| f32_at(m, a));
        let torque = read(m, w.torque_at);
        let quantity = read(m, w.quantity_at);
        let mut kmh = 0.0;
        if let Some(c) = self.crank.as_mut().filter(|_| !self.held) {
            let was = self.rpm;
            if let Some(v) = &self.vehicle {
                let (rpm, k) = v.step(c, torque, dt);
                self.rpm = rpm;
                kmh = k;
                if let Some(r) = &self.road {
                    r.set(kmh);
                }
            } else {
                self.rpm = c.step(torque, dt);
            }
            if self.rpm != was {
                m.chip.stimulus_set("rpm", self.rpm);
            }
        } else if let Some(v) = &self.vehicle {
            kmh = v.kmh_at(self.rpm);
            if let Some(r) = &self.road {
                r.set(kmh);
            }
        }
        let duty = |(c, k): (usize, usize)| m.chip.pwm_duty(c, k);
        let scv_duty = duty(w.scv);
        let vgt_duty = duty(w.vgt);
        let egr_drive = duty(w.egr_open).unwrap_or(0.0) - duty(w.egr_close).unwrap_or(0.0);
        self.scv.step(scv_duty.unwrap_or(0.0), dt);
        let egr_sensor = self.egr.step(egr_drive, dt);
        let egr = egr_sensor.max(0.0);
        m.chip.adc.input[self.scv_in.channel] = self.scv_in.code(self.scv.milliamps());
        m.chip.adc.input[self.egr_in.channel] = self.egr_in.code(egr_sensor);
        let rail = if let Some(addr) = w.rail_asked_at {
            let asked = f32_at(m, addr);
            if asked.is_finite() && asked >= 0.0 {
                let p = self.rail.step(asked, dt);
                m.chip.adc.input[self.rail_in.channel] = self.rail_in.code(p);
                p
            } else {
                f64::NAN
            }
        } else {
            f64::NAN
        };
        // The turbine on the fuel the firmware asks for, with its vanes where it puts them;
        // no boost where the fuel is not known.
        let burnt = w.quantity_at.map_or(0.0, |_| quantity);
        let vgt = vgt_duty.unwrap_or(0.0) * 100.0;
        let boost = self.rest_kpa + self.turbo.step(self.rpm, burnt, vgt, dt);
        m.chip.adc.input[self.boost_in.channel] = self.boost_in.code(boost);
        let maf = if self.rpm > 0.0 {
            let g = crate::air::fresh_air_g_per_s(self.rpm, boost, self.intake_c, egr);
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
            quantity,
            rail,
            kmh,
        }
    }
}
