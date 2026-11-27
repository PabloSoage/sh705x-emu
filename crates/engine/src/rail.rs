//! High-pressure fuel rail: a first-order, rate-limited follower of the firmware's
//! setpoint (`rail_asked`).
//!
//! In the car (`scanmatik-bench` routes 72 and 109), the rail sensor tracks the desired
//! rail pressure (`0x308A`) within a fraction of a MPa with `tau ≈ 0.23 s` and a slew
//! rate under 600 MPa/s [DER]. This idealised plant closes the rail pressure sensor loop
//! so the firmware sees realistic rail pressure without modelling the high-pressure pump.

/// A high-pressure fuel rail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rail {
    /// Time constant to reach the setpoint, seconds [DER].
    pub tau: f64,
    /// Fastest rise, MPa/s [DER].
    pub rise: f64,
    /// Fastest fall, MPa/s [DER].
    pub fall: f64,
    /// Pressure now, MPa.
    pub p: f64,
}

impl Rail {
    /// The Astra J A17DTJ's rail, from the car's logs.
    pub fn astra() -> Self {
        Self {
            tau: 0.23,
            rise: 600.0,
            fall: 600.0,
            p: 0.0,
        }
    }

    /// Advances `dt` seconds toward `setpoint` (MPa) and returns the pressure, MPa.
    pub fn step(&mut self, setpoint: f64, dt: f64) -> f64 {
        let want = (setpoint - self.p) / self.tau;
        let rate = want.clamp(-self.fall, self.rise);
        self.p = (self.p + rate * dt).max(0.0);
        self.p
    }
}

impl Default for Rail {
    fn default() -> Self {
        Self::astra()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reaches_the_setpoint() {
        let mut r = Rail::astra();
        for _ in 0..500 {
            r.step(24.0, r.tau / 100.0);
        }
        assert!((r.p - 24.0).abs() < 0.24, "{}", r.p);
    }

    #[test]
    fn it_is_rate_limited() {
        let mut r = Rail::astra();
        r.p = 24.0;
        r.step(150.0, 1e-3);
        assert!(r.p <= 24.0 + r.rise * 1e-3 + 1e-9, "{}", r.p);

        let mut r = Rail::astra();
        r.p = 1.0;
        r.step(0.0, 1.0);
        assert_eq!(r.p, 0.0);
    }
}
