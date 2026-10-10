//! The actuators the ECU drives with a PWM output and reads back on an analog input:
//! the suction control valve of the high-pressure pump (a current loop) and the EGR
//! valve (a position loop). Each is a small physical plant turning a duty fraction into
//! the quantity the firmware measures, so the firmware's own loop can close against it.
//!
//! The firmware drives the SCV on ATU-II output 7A and reads its current back on AN1
//! (`scv-feedback`, mA); it drives the EGR on 7C and reads the valve's position on AN10
//! (`egr`, %). Which output is which was found by perturbing one input at a time and
//! watching the duties move (resultados fase 5 §6) [MED]; the plants' electrical and
//! mechanical constants are not in the flash, so they are plausible values [SUP] and the
//! validation is that the firmware's loop settles where its own setpoints ask, not the
//! constants themselves.

/// A current-controlled solenoid: the SCV coil across the battery, switched at the PWM
/// duty. The current follows `dI/dt = (duty·V − R·I) / L`, so it settles at `duty·V/R`.
#[derive(Clone, Debug)]
pub struct Solenoid {
    /// Coil resistance, ohm [SUP].
    pub r: f64,
    /// Coil inductance, henry [SUP].
    pub l: f64,
    /// Supply across the coil, volt.
    pub v: f64,
    /// Current now, ampere.
    pub i: f64,
}

impl Solenoid {
    /// The SCV coil: 4 Ω and 6 mH [SUP] across 14 V give a full-duty current of 3.5 A,
    /// the top of the AN1 curve (3.57 A), and 1.73 A — the car's warm-idle reading — at a
    /// duty of about 0.49.
    pub fn scv() -> Self {
        Self {
            r: 4.0,
            l: 6e-3,
            v: 14.0,
            i: 0.0,
        }
    }

    /// Advances `dt` seconds at `duty` (0..1) and returns the current, ampere. The coil's
    /// response to a constant duty is first order, so the step is taken in closed form and
    /// is exact and stable for any `dt`.
    pub fn step(&mut self, duty: f64, dt: f64) -> f64 {
        let steady = self.steady(duty);
        let a = (-dt * self.r / self.l).exp();
        self.i = steady + (self.i - steady) * a;
        self.i
    }

    /// The current it settles at for `duty`, ampere.
    pub fn steady(&self, duty: f64) -> f64 {
        duty.clamp(0.0, 1.0) * self.v / self.r
    }

    /// The current now in milliampere, the unit of the `scv-feedback` curve.
    pub fn milliamps(&self) -> f64 {
        self.i * 1000.0
    }
}

/// A position actuator with a return spring: the EGR valve opens toward `100·duty` % with
/// a first-order lag, and the spring shuts it when the duty falls. The firmware reads the
/// position on AN10 and closes its own loop onto the desired-EGR setpoint.
#[derive(Clone, Debug)]
pub struct Egr {
    /// Time constant of the valve, second [SUP].
    pub tau: f64,
    /// Open fraction reached at full duty, % of travel per unit duty [SUP].
    pub gain: f64,
    /// Position now, % open.
    pub pos: f64,
}

impl Egr {
    /// The EGR valve: a 120 ms travel [SUP], fully open at full duty.
    pub fn egr() -> Self {
        Self {
            tau: 0.12,
            gain: 100.0,
            pos: 0.0,
        }
    }

    /// Advances `dt` seconds at `duty` (0..1) and returns the position, % open.
    pub fn step(&mut self, duty: f64, dt: f64) -> f64 {
        let target = (self.gain * duty.clamp(0.0, 1.0)).clamp(0.0, 100.0);
        // Exact first-order response over the step, stable for any dt.
        let a = (-dt / self.tau).exp();
        self.pos = target + (self.pos - target) * a;
        self.pos
    }

    /// The position it settles at for `duty`, % open.
    pub fn steady(&self, duty: f64) -> f64 {
        (self.gain * duty.clamp(0.0, 1.0)).clamp(0.0, 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scv_current_settles_at_duty_times_v_over_r() {
        let mut s = Solenoid::scv();
        // Full duty reaches the top of the feedback curve.
        for _ in 0..100 {
            s.step(1.0, 1e-3);
        }
        assert!((s.i - 3.5).abs() < 0.01, "{}", s.i);
        assert!((s.milliamps() - 3500.0).abs() < 10.0);
        // The warm-idle reading of 1729 mA asks for a duty near 0.49.
        let duty = 1.729 / s.steady(1.0);
        assert!((duty - 0.494).abs() < 0.01, "{duty}");
        let mut s = Solenoid::scv();
        for _ in 0..200 {
            s.step(duty, 1e-3);
        }
        assert!((s.milliamps() - 1729.0).abs() < 5.0, "{}", s.milliamps());
    }

    #[test]
    fn scv_current_rises_within_a_few_time_constants() {
        let mut s = Solenoid::scv();
        let tau = s.l / s.r; // 1.5 ms
        s.step(1.0, tau); // one time constant: about 63 %
        assert!((s.i / s.steady(1.0) - 0.632).abs() < 0.02, "{}", s.i);
        // A big step in one call is as stable as many small ones.
        let mut coarse = Solenoid::scv();
        coarse.step(1.0, 1.0);
        assert!((coarse.i - s.steady(1.0)).abs() < 1e-6);
    }

    #[test]
    fn egr_opens_and_the_spring_shuts_it() {
        let mut e = Egr::egr();
        // Half duty settles at half travel.
        for _ in 0..100 {
            e.step(0.5, 1e-2);
        }
        assert!((e.pos - 50.0).abs() < 0.1, "{}", e.pos);
        // One time constant toward a closed valve: about 37 % of the way left.
        e.step(0.0, e.tau);
        assert!((e.pos - 50.0 * 0.368).abs() < 0.5, "{}", e.pos);
        // And it shuts fully.
        for _ in 0..100 {
            e.step(0.0, 1e-2);
        }
        assert!(e.pos < 0.1, "{}", e.pos);
    }

    #[test]
    fn egr_response_is_stable_for_any_step() {
        // The exact first-order form never overshoots, whatever the step.
        let mut e = Egr::egr();
        e.step(1.0, 10.0);
        assert!((e.pos - 100.0).abs() < 1e-6);
        let mut e = Egr::egr();
        for _ in 0..1000 {
            e.step(1.0, 1e-4);
        }
        assert!(e.pos <= 100.0 + 1e-9);
    }
}
