//! Air through the engine: a speed-density estimate of the mass air flow.
//!
//! A four-stroke engine draws its displacement every two revolutions; the air's density
//! at the manifold's pressure and temperature, times a volumetric efficiency, gives the
//! mass. The A17DTJ displaces 1686 cm³ [CIT]; the efficiency is taken as 0.85 for the
//! whole range [SUP].

/// Displacement, m³.
pub const DISPLACEMENT: f64 = 1686e-6;
/// Volumetric efficiency [SUP].
pub const VOLUMETRIC_EFFICIENCY: f64 = 0.85;
/// Specific gas constant of dry air, J/(kg·K).
const R_AIR: f64 = 287.05;

/// Mass air flow, g/s, at `rpm` with the manifold at `kpa` and `deg_c`.
pub fn maf_g_per_s(rpm: f64, kpa: f64, deg_c: f64) -> f64 {
    let density = kpa * 1000.0 / (R_AIR * (deg_c + 273.15));
    let volume = DISPLACEMENT * rpm.max(0.0) / 120.0;
    volume * density * VOLUMETRIC_EFFICIENCY * 1000.0
}

/// The share of the charge that is exhaust gas with the EGR valve `egr_pct` % open:
/// `0.66·(1 − e^(−egr/30))`. Fitted to the car standing at a warm idle (scanmatik-bench
/// routes 67, 69 and 72, 700–800 rpm, coolant at 70 °C or more), where the fresh air read
/// 5.60, 5.16 and 4.44 g/s with the valve at 27.5, 32.5 and 47.5 % (695, 6140 and 46
/// samples), against [`maf_g_per_s`]'s 9.2 g/s of charge at 59 °C [DER]. With the valve
/// shut the car read 7.87 g/s (188 samples), 15 % under this model [MED]: the efficiency
/// or a transient, not settled.
pub fn egr_fraction(egr_pct: f64) -> f64 {
    0.66 * (1.0 - (-egr_pct.max(0.0) / 30.0).exp())
}

/// The fresh air the MAF reads, g/s: the charge at `rpm`, `kpa` and `deg_c` less the
/// exhaust gas let in by the EGR valve at `egr_pct` %.
pub fn fresh_air_g_per_s(rpm: f64, kpa: f64, deg_c: f64, egr_pct: f64) -> f64 {
    maf_g_per_s(rpm, kpa, deg_c) * (1.0 - egr_fraction(egr_pct))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_and_full_speed() {
        // 750 rpm at 98 kPa and 26 degC: about 10 g/s (the car reads 5.5 at idle, with a
        // third of its intake being exhaust gas through the EGR).
        let idle = maf_g_per_s(750.0, 98.0, 26.0);
        assert!((idle - 10.2).abs() < 0.2, "{idle}");
        // Twice the speed, twice the air; twice the pressure, twice the air.
        assert!((maf_g_per_s(1500.0, 98.0, 26.0) - 2.0 * idle).abs() < 1e-9);
        assert!((maf_g_per_s(750.0, 196.0, 26.0) - 2.0 * idle).abs() < 1e-9);
        assert_eq!(maf_g_per_s(0.0, 98.0, 26.0), 0.0);
    }

    #[test]
    fn the_egr_takes_its_share_of_the_charge() {
        let charge = maf_g_per_s(750.0, 98.0, 59.0);
        assert_eq!(fresh_air_g_per_s(750.0, 98.0, 59.0, 0.0), charge);
        // Near the car's warm idle: a third open, about 5.2 g/s of fresh air.
        let idle = fresh_air_g_per_s(750.0, 98.0, 59.0, 32.5);
        assert!((idle - 5.2).abs() < 0.2, "{idle}");
        // More opening, less fresh air, never below a third of the charge.
        assert!(fresh_air_g_per_s(750.0, 98.0, 59.0, 47.5) < idle);
        assert!(fresh_air_g_per_s(750.0, 98.0, 59.0, 100.0) > 0.33 * charge);
    }
}
