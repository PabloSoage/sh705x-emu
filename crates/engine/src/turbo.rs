//! The turbocharger: the boost a variable-geometry turbine gives for the exhaust it gets
//! and where the firmware puts its vanes.
//!
//! What the car does: the median boost over the barometer by engine speed and fuel per
//! stroke, from scanmatik-bench routes 67, 69 and 72 ([`BOOST`]) [MED], with the vanes where
//! the car's own loop put them, which is lower the faster the engine turns
//! ([`typical_vgt`]).
//!
//! The vanes' own effect cannot be read from those logs: the car's loop moves them to
//! hold its setpoint, so in its data more command goes with less boost. Its sign is the
//! firmware's: with the boost rising to its setpoint it lowers the command (resultados
//! fase 8 §4), so more command closes the vanes and gives more boost. Here the boost
//! scales as `(command / typical)^2` [SUP], and follows its steady value with a 0.4 s lag
//! [SUP].

/// Engine speeds of [`BOOST`]'s rows, rpm.
const SPEEDS: [f64; 7] = [1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 3500.0, 4000.0];
/// Fuel per stroke of its columns, mm³.
const FUELS: [f64; 7] = [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0];
/// The car's median boost over the barometer, kPa, in bands of 500 rpm and 10 mm³ around
/// each point, where a band had 200 samples or more [MED]; the others interpolated or
/// carried from the next band, marked `*` [DER]; at 3000 rpm and 30 mm³, 206 samples read
/// 30.6 kPa, out of line with both neighbours, and the mean of these is used. With no
/// fuel the boost is taken as none: what the car shows then is the lag of what came
/// before.
#[rustfmt::skip]
const BOOST: [[f64; 7]; 7] = [
    //  0     10     20     30     40     50     60 mm³
    [0.0,   0.0,   3.5,   7.0,  14.5,  25.0, 35.0], // 1000 rpm (50*, 60*)
    [0.0,   2.4,  11.8,  34.1,  48.9,  75.6, 118.5], // 1500
    [0.0,   7.8,  24.9,  56.7,  86.2, 112.0, 126.4], // 2000
    [0.0,  13.9,  42.1,  79.5, 112.0, 121.3, 136.6], // 2500
    [0.0,   9.2,  43.5,  79.9, 107.1, 140.0, 141.0], // 3000 (10*, 20*, 30*)
    [0.0,   4.4,  44.9,  80.3, 107.7, 133.6, 137.0], // 3500 (60*)
    [0.0,  23.6,  48.6,  77.4, 100.2, 122.9, 130.0], // 4000 (60*)
];

/// The car's boost over the barometer with its own vanes, kPa, interpolated in speed and
/// fuel and held at the edges.
pub fn car_boost(rpm: f64, q: f64) -> f64 {
    let at = |axis: &[f64], x: f64| {
        let x = x.clamp(axis[0], axis[axis.len() - 1]);
        let i = axis
            .iter()
            .rposition(|&a| a <= x)
            .unwrap_or(0)
            .min(axis.len() - 2);
        (i, (x - axis[i]) / (axis[i + 1] - axis[i]))
    };
    let (i, fi) = at(&SPEEDS, rpm);
    let (j, fj) = at(&FUELS, q);
    let row = |r: usize| BOOST[r][j] + (BOOST[r][j + 1] - BOOST[r][j]) * fj;
    row(i) + (row(i + 1) - row(i)) * fi
}

/// The car's VGT command, %, by engine speed, rpm: the mean of route 72 and 67 in bands
/// of 500 rpm [MED].
const TYPICAL: [(f64, f64); 8] = [
    (1000.0, 66.6),
    (1500.0, 59.0),
    (2000.0, 56.4),
    (2500.0, 55.1),
    (3000.0, 51.0),
    (3500.0, 37.7),
    (4000.0, 36.0),
    (4500.0, 27.9),
];

/// The car's usual VGT command at `rpm`, %, interpolated and held at the ends.
pub fn typical_vgt(rpm: f64) -> f64 {
    let (first, last) = (TYPICAL[0], TYPICAL[TYPICAL.len() - 1]);
    if rpm <= first.0 {
        return first.1;
    }
    if rpm >= last.0 {
        return last.1;
    }
    let i = TYPICAL.iter().position(|&(r, _)| r > rpm).unwrap_or(1);
    let ((r0, v0), (r1, v1)) = (TYPICAL[i - 1], TYPICAL[i]);
    v0 + (v1 - v0) * (rpm - r0) / (r1 - r0)
}

/// Most boost over the barometer it gives, kPa: a little over the car's highest (142 kPa
/// in the 2500 rpm band) [DER].
pub const MAX_BOOST: f64 = 150.0;

/// The turbocharger.
#[derive(Clone, Debug)]
pub struct Turbo {
    /// Lag, seconds [SUP].
    pub tau: f64,
    /// Boost over the barometer now, kPa.
    pub boost: f64,
}

impl Turbo {
    /// A turbocharger at rest.
    pub fn new() -> Self {
        Self {
            tau: 0.4,
            boost: 0.0,
        }
    }

    /// The boost over the barometer it settles at, kPa, at `rpm` with `q` mm³ a stroke and
    /// the VGT command at `vgt` %.
    pub fn steady(rpm: f64, q: f64, vgt: f64) -> f64 {
        if rpm <= 0.0 || q <= 0.0 {
            return 0.0;
        }
        let gas = car_boost(rpm, q);
        let vanes = (vgt.max(0.0) / typical_vgt(rpm)).powi(2);
        (gas * vanes).min(MAX_BOOST)
    }

    /// Advances `dt` seconds and returns the boost over the barometer, kPa. The lag is
    /// taken in closed form, stable for any `dt`.
    pub fn step(&mut self, rpm: f64, q: f64, vgt: f64, dt: f64) -> f64 {
        let target = Self::steady(rpm, q, vgt);
        self.boost = target + (self.boost - target) * (-dt / self.tau).exp();
        self.boost
    }
}

impl Default for Turbo {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cars_boost_with_its_own_vanes() {
        // 2000 rpm and 40 mm³ with the car's vanes: about 110 kPa, as in its logs.
        let b = Turbo::steady(2000.0, 40.0, typical_vgt(2000.0));
        assert!((b - 110.0).abs() < 25.0, "{b}");
        // At idle there is next to none.
        assert!(Turbo::steady(750.0, 7.0, 60.0) < 3.0);
        // No fuel or no speed, no boost; never past the top.
        assert_eq!(Turbo::steady(2000.0, -10.0, 60.0), 0.0);
        assert_eq!(Turbo::steady(0.0, 40.0, 60.0), 0.0);
        assert_eq!(Turbo::steady(4000.0, 80.0, 100.0), MAX_BOOST);
    }

    #[test]
    fn closing_the_vanes_raises_it() {
        let open = Turbo::steady(2000.0, 30.0, 40.0);
        let closed = Turbo::steady(2000.0, 30.0, 70.0);
        assert!(closed > open * 2.0, "{open} {closed}");
    }

    #[test]
    fn it_lags() {
        let mut t = Turbo::new();
        let target = Turbo::steady(2000.0, 40.0, 56.4);
        t.step(2000.0, 40.0, 56.4, t.tau);
        assert!((t.boost / target - 0.632).abs() < 0.01, "{}", t.boost);
        let mut big = Turbo::new();
        big.step(2000.0, 40.0, 56.4, 100.0);
        assert!((big.boost - target).abs() < 1e-9);
    }

    #[test]
    fn typical_command_interpolates() {
        assert_eq!(typical_vgt(500.0), 66.6);
        assert!((typical_vgt(1250.0) - 62.8).abs() < 1e-9);
        assert_eq!(typical_vgt(9000.0), 27.9);
    }
}
