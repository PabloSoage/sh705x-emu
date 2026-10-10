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
}
