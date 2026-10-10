//! The crankshaft as a rotating mass: engine speed from the torque the firmware asks for.
//!
//! The firmware computes the torque it delivers (GMLAN 0x1A2D) net of its own model of
//! the engine's friction: with no fuel at all it reports about -24 N·m, the drag of a
//! turning engine [MED]. So the crank here only adds what the firmware does not know
//! about, an external load (the alternator, the air conditioning, a gearbox in gear),
//! and integrates `J·dω/dt = T - load`.
//!
//! The inertia of the A17DTJ's crank, flywheel and clutch is not known; 0.2 kg·m² is
//! taken, the order of a 1.7 l diesel with a dual-mass flywheel [SUP].

/// Below this speed the engine stalls, rpm [SUP].
pub const STALL_RPM: f64 = 300.0;

/// A crankshaft turning freely, but for an external load.
#[derive(Clone, Debug, PartialEq)]
pub struct Crank {
    /// Moment of inertia of everything turning with the crank, kg·m².
    pub inertia: f64,
    /// External load torque, N·m.
    pub load: f64,
    /// Speed, rpm.
    pub rpm: f64,
}

impl Crank {
    /// The A17DTJ's crank [SUP], turning at `rpm` with no external load.
    pub fn new(rpm: f64) -> Self {
        Self {
            inertia: 0.2,
            load: 0.0,
            rpm,
        }
    }

    /// Turns for `dt` seconds under the engine's net `torque`, N·m, held over the step
    /// as the firmware holds it between its updates; returns the new speed. Once below
    /// [`STALL_RPM`] the engine stops, and stays stopped.
    pub fn step(&mut self, torque: f64, dt: f64) -> f64 {
        if self.rpm == 0.0 {
            return 0.0;
        }
        let alpha = (torque - self.load) / self.inertia; // rad/s²
        self.rpm += alpha * dt * 60.0 / std::f64::consts::TAU;
        if self.rpm < STALL_RPM {
            self.rpm = 0.0;
        }
        self.rpm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torque_accelerates_and_load_brakes() {
        // 20 N·m on 0.2 kg·m² for a second: 100 rad/s, about 955 rpm more.
        let mut c = Crank::new(800.0);
        for _ in 0..1000 {
            c.step(20.0, 1e-3);
        }
        assert!((c.rpm - 800.0 - 100.0 * 60.0 / std::f64::consts::TAU).abs() < 1e-6);
        // A load equal to the torque holds the speed.
        let mut c = Crank::new(800.0);
        c.load = 15.0;
        c.step(15.0, 0.5);
        assert_eq!(c.rpm, 800.0);
    }

    #[test]
    fn it_stalls_and_stays_stalled() {
        let mut c = Crank::new(400.0);
        c.step(-24.0, 0.1);
        assert_eq!(c.rpm, 0.0);
        assert_eq!(c.step(100.0, 0.1), 0.0, "a stalled engine does not restart");
    }
}
