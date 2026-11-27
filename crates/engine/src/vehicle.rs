//! Vehicle longitudinal load and gearbox: maps engine torque to crankshaft speed and
//! road speed for a car in neutral or in gear.
//!
//! In neutral (`gear == 0`), the engine sees only its own rotating inertia and auxiliary
//! load (`aux_load_nm`), matching [`Crank`]. In gear (`1..=6`), the vehicle's mass
//! reflects to the crankshaft as `m · (r / i)²` and rolling resistance, grade and
//! aerodynamic drag reflect as road-load torque.

use crate::crank::Crank;

/// Standard gravity, m/s².
const G: f64 = 9.81;

/// A vehicle and its manual gearbox coupled to the crankshaft.
#[derive(Clone, Debug, PartialEq)]
pub struct Vehicle {
    /// Vehicle mass including driver/fuel, kg.
    pub mass_kg: f64,
    /// Rolling resistance coefficient.
    pub c_rr: f64,
    /// Drag area `Cd · A`, m².
    pub cd_a: f64,
    /// Air density, kg/m³.
    pub rho_air: f64,
    /// Loaded wheel radius, m (2.0 m circumference [MED] from `bus::KMH_A_PULSE_A_MS`).
    pub wheel_radius_m: f64,
    /// Final drive ratio (MZ4 3.65).
    pub final_drive: f64,
    /// Gearbox ratios for gears 1..=6 (measured from `scanmatik-bench` route 73 [MED]).
    pub gears: [f64; 6],
    /// Drivetrain mechanical efficiency in gear.
    pub drivetrain_eff: f64,
    /// Auxiliary load on the crank (alternator, A/C, pump), N·m.
    pub aux_load_nm: f64,
    /// Engine + flywheel + clutch inertia, kg·m².
    pub engine_inertia: f64,
    /// Road grade (`sin(θ)`, positive uphill).
    pub grade: f64,
    /// Selected gear: `0` for neutral, `1..=6` for forward gears.
    pub gear: u8,
}

impl Vehicle {
    /// The Astra J A17DTJ (MZ4 6-speed gearbox, 3.65 final drive) in `gear` (`0` = neutral).
    ///
    /// Only the gear ratios and the wheel circumference come from the car. Mass, rolling
    /// resistance, drag area, drivetrain efficiency, auxiliary load and engine inertia are
    /// assumed values for a car of this class, not measured. The ratios were measured with
    /// the original gearbox; 1st reads 4.04 where the M32's nominal is 3.82.
    pub fn astra(gear: u8) -> Self {
        Self {
            mass_kg: 1500.0,
            c_rr: 0.012,
            cd_a: 0.72,
            rho_air: 1.225,
            wheel_radius_m: 2.0 / std::f64::consts::TAU,
            final_drive: 3.65,
            gears: [4.04, 2.07, 1.30, 0.95, 0.74, 0.61],
            drivetrain_eff: 0.90,
            aux_load_nm: 12.0,
            engine_inertia: 0.2,
            grade: 0.0,
            gear,
        }
    }

    /// Overall transmission ratio `gear * final_drive`, or `None` in neutral / invalid gear.
    pub fn total_ratio(&self) -> Option<f64> {
        let idx = self.gear.checked_sub(1)? as usize;
        self.gears.get(idx).map(|&g| g * self.final_drive)
    }

    /// Road speed in km/h at `rpm`, or `0.0` in neutral.
    pub fn kmh_at(&self, rpm: f64) -> f64 {
        let Some(ratio) = self.total_ratio() else {
            return 0.0;
        };
        let omega = rpm.max(0.0) * std::f64::consts::TAU / 60.0;
        let v_ms = omega * (self.wheel_radius_m / ratio);
        v_ms * 3.6
    }

    /// Engine speed in rpm at `kmh`, or `None` in neutral.
    pub fn rpm_at(&self, kmh: f64) -> Option<f64> {
        let ratio = self.total_ratio()?;
        let v_ms = kmh.max(0.0) / 3.6;
        let omega = v_ms * ratio / self.wheel_radius_m;
        Some(omega * 60.0 / std::f64::consts::TAU)
    }

    /// Equivalent rotational inertia at the crankshaft, kg·m².
    pub fn effective_inertia(&self) -> f64 {
        match self.total_ratio() {
            Some(ratio) => {
                let r_eff = self.wheel_radius_m / ratio;
                self.engine_inertia + self.mass_kg * r_eff * r_eff
            }
            None => self.engine_inertia,
        }
    }

    /// Total load torque reflected to the crankshaft at `rpm`, N·m.
    pub fn road_load_torque(&self, rpm: f64) -> f64 {
        let Some(ratio) = self.total_ratio() else {
            return self.aux_load_nm;
        };
        let v_ms = self.kmh_at(rpm) / 3.6;
        let f_roll = self.mass_kg * G * (self.c_rr + self.grade);
        let f_aero = 0.5 * self.rho_air * self.cd_a * v_ms * v_ms;
        let f_road = f_roll + f_aero;
        self.aux_load_nm + f_road * self.wheel_radius_m / (ratio * self.drivetrain_eff)
    }

    /// Advances `crank` by `dt` seconds under engine net `torque` (N·m) and returns
    /// `(rpm, kmh)`.
    pub fn step(&self, crank: &mut Crank, torque: f64, dt: f64) -> (f64, f64) {
        crank.inertia = self.effective_inertia();
        crank.load = self.road_load_torque(crank.rpm);
        let rpm = crank.step(torque, dt);
        (rpm, self.kmh_at(rpm))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_matches_crank() {
        let v = Vehicle::astra(0);
        assert_eq!(v.total_ratio(), None);
        assert_eq!(v.effective_inertia(), v.engine_inertia);
        assert_eq!(v.road_load_torque(2000.0), v.aux_load_nm);
        assert_eq!(v.kmh_at(2000.0), 0.0);

        let mut c1 = Crank::new(800.0);
        c1.load = v.aux_load_nm;
        let mut c2 = Crank::new(800.0);
        c1.step(20.0, 0.01);
        let (rpm, kmh) = v.step(&mut c2, 20.0, 0.01);
        assert_eq!(rpm, c1.rpm);
        assert_eq!(kmh, 0.0);
    }

    #[test]
    fn higher_gears_give_higher_speed_and_lower_reflected_inertia() {
        let v3 = Vehicle::astra(3);
        let v4 = Vehicle::astra(4);
        let v6 = Vehicle::astra(6);
        assert!(v4.kmh_at(2000.0) > v3.kmh_at(2000.0));
        assert!(v6.kmh_at(2000.0) > v4.kmh_at(2000.0));
        // 4th gear at 2900 rpm is ~100 km/h (route 73: ~29 rpm/(km/h)).
        assert!((v4.kmh_at(2900.0) - 100.0).abs() < 2.0);
        assert!((v4.rpm_at(100.0).unwrap() - 2900.0).abs() < 60.0);
        // Higher gear = smaller ratio = larger reflected inertia m*(r/i)^2.
        assert!(v4.effective_inertia() > v3.effective_inertia());
    }

    #[test]
    fn positive_torque_accelerates_and_coasting_decelerates() {
        let v = Vehicle::astra(4);
        let mut c = Crank::new(2000.0);
        let start_kmh = v.kmh_at(c.rpm);
        for _ in 0..3000 {
            v.step(&mut c, 150.0, 1e-3);
        }
        assert!(v.kmh_at(c.rpm) > start_kmh + 5.0, "rpm={}", c.rpm);

        let high_rpm = c.rpm;
        for _ in 0..3000 {
            v.step(&mut c, -24.0, 1e-3);
        }
        assert!(c.rpm < high_rpm - 50.0, "rpm={}", c.rpm);
    }
}
