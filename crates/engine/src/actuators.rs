//! The actuators the ECU drives with a PWM output and reads back on an analog input:
//! the suction control valve of the high-pressure pump (a current loop) and the EGR
//! valve (a position loop). Each is a small physical plant turning a duty fraction into
//! the quantity the firmware measures, so the firmware's own loop can close against it.
//!
//! The firmware drives the SCV on ATU-II output 6A and reads its current back on AN1
//! (`scv-feedback`, mA); it drives the EGR through an H-bridge, opening on 7D and closing
//! on 7C, and reads the valve's position on AN10 (`egr`, %). Which output is which was
//! found in the code that writes each output (resultados fase 9; fase 5 §6 took 7A, the
//! VGT, for the SCV) [MED]. The plants' electrical and mechanical constants are not in
//! the flash, so they are plausible values [SUP] and the validation is that the
//! firmware's loop settles where its own setpoints ask, not the constants themselves.

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
    /// The SCV coil: 3.33 Ω, so that the car's warm-idle command of 41.2 % gives its
    /// 1729 mA across 14 V (scanmatik-bench route 72) [DER]; 6 mH [SUP].
    pub fn scv() -> Self {
        Self {
            r: 3.33,
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

/// The EGR valve: a DC motor against a return spring, driven through an H-bridge whose
/// halves are two PWM outputs, one opening and one closing (7D and 7C, resultados fase 9)
/// [MED]. The valve moves at a speed proportional to the net drive beyond what holds it
/// against its spring, and stops at its ends; with no drive the spring shuts it.
///
/// The holding drive is the car's: standing at any opening from 10 to 100 %, its EGR
/// command (0x32A8, signed) has a mean of 10 to 15 % (scanmatik-bench route 72, 160,000
/// samples) [DER]. The speed is a plausible value [SUP]: the car's logs are sampled too
/// coarsely to time a full stroke.
#[derive(Clone, Debug)]
pub struct Egr {
    /// Travel speed per unit of net drive beyond the hold, % a second [SUP].
    pub rate: f64,
    /// Drive that holds the valve still against its spring, fraction of full.
    pub hold: f64,
    /// Position now, % open.
    pub pos: f64,
}

impl Egr {
    /// The EGR valve, shut: 1000 %/s a unit of drive [SUP], held by 0.13 [DER].
    pub fn new() -> Self {
        Self {
            rate: 1000.0,
            hold: 0.13,
            pos: 0.0,
        }
    }

    /// Advances `dt` seconds with `drive` (opening minus closing duty, -1..1) and
    /// returns the position, % open.
    pub fn step(&mut self, drive: f64, dt: f64) -> f64 {
        let net = drive.clamp(-1.0, 1.0) - self.hold;
        self.pos = (self.pos + self.rate * net * dt).clamp(0.0, 100.0);
        self.pos
    }
}

impl Default for Egr {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scv_current_settles_at_duty_times_v_over_r() {
        let mut s = Solenoid::scv();
        // Full duty: V/R.
        for _ in 0..100 {
            s.step(1.0, 1e-3);
        }
        assert!((s.i - 14.0 / 3.33).abs() < 0.01, "{}", s.i);
        // The car's warm idle: 1729 mA at 41.2 %.
        let duty = 1.729 / s.steady(1.0);
        assert!((duty - 0.412).abs() < 0.005, "{duty}");
        let mut s = Solenoid::scv();
        for _ in 0..200 {
            s.step(duty, 1e-3);
        }
        assert!((s.milliamps() - 1729.0).abs() < 5.0, "{}", s.milliamps());
    }

    #[test]
    fn scv_current_rises_within_a_few_time_constants() {
        let mut s = Solenoid::scv();
        let tau = s.l / s.r; // 1.8 ms
        s.step(1.0, tau); // one time constant: about 63 %
        assert!((s.i / s.steady(1.0) - 0.632).abs() < 0.02, "{}", s.i);
        // A big step in one call is as stable as many small ones.
        let mut coarse = Solenoid::scv();
        coarse.step(1.0, 1.0);
        assert!((coarse.i - s.steady(1.0)).abs() < 1e-6);
    }

    #[test]
    fn egr_holds_at_its_holding_drive() {
        let mut e = Egr::new();
        e.pos = 34.0;
        for _ in 0..1000 {
            e.step(e.hold, 1e-3);
        }
        assert!((e.pos - 34.0).abs() < 1e-9, "{}", e.pos);
    }

    #[test]
    fn egr_opens_on_drive_and_the_spring_shuts_it() {
        let mut e = Egr::new();
        // Full opening drive: (1 - hold) x rate, 87 % in 100 ms.
        e.step(1.0, 0.1);
        assert!((e.pos - 87.0).abs() < 1e-9, "{}", e.pos);
        // It stops at the end.
        e.step(1.0, 1.0);
        assert_eq!(e.pos, 100.0);
        // No drive: the spring alone shuts it, at hold x rate.
        e.step(0.0, 0.5);
        assert!((e.pos - 35.0).abs() < 1e-9, "{}", e.pos);
        // Closing drive is faster, and it stops shut.
        e.step(-1.0, 0.03);
        assert!((e.pos - 1.1).abs() < 1e-9, "{}", e.pos);
        e.step(-1.0, 1.0);
        assert_eq!(e.pos, 0.0);
        // A drive past full counts as full.
        e.step(5.0, 0.01);
        assert!((e.pos - 8.7).abs() < 1e-9, "{}", e.pos);
    }
}
