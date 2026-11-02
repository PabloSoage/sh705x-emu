//! Full-throttle dyno pull simulation (Power and Torque curves vs RPM).

use crate::runner::{run_scenario, ScenarioOptions};
use std::fmt::Write as _;

pub struct DynoOptions {
    pub flash_a: String,
    pub flash_b: Option<String>,
    pub start_rpm: f64,
    pub end_rpm: f64,
    pub step_rpm: f64,
    pub pedal_pct: f64,
    pub gear: u8,
    pub millis: u64,
}

impl Default for DynoOptions {
    fn default() -> Self {
        Self {
            flash_a: "flash.bin".into(),
            flash_b: None,
            start_rpm: 1200.0,
            end_rpm: 4400.0,
            step_rpm: 200.0,
            pedal_pct: 100.0,
            gear: 4,
            millis: 450,
        }
    }
}

pub fn run_dyno(opts: &DynoOptions) -> Result<String, String> {
    let mut out = String::new();

    writeln!(
        out,
        "=== Simulated Dyno Pull (WOT @ {:.0}% Pedal, Gear {}) ===",
        opts.pedal_pct, opts.gear
    )
    .unwrap();
    writeln!(out, "Flash A: {}", opts.flash_a).unwrap();
    if let Some(ref fb) = opts.flash_b {
        writeln!(out, "Flash B (Compare): {}", fb).unwrap();
    }
    writeln!(out).unwrap();

    writeln!(
        out,
        "{:>6} | {:>10} | {:>9} | {:>9} | {:>9} | {:>10} | {:>10}",
        "RPM", "Torque(Nm)", "Power(kW)", "Power(HP)", "Fuel(mm3)", "Boost(kPa)", "Rail(MPa)"
    )
    .unwrap();
    writeln!(
        out,
        "-----------------------------------------------------------------------------"
    )
    .unwrap();

    let mut current_rpm = opts.start_rpm;
    while current_rpm <= opts.end_rpm + 1.0 {
        let sc_a = ScenarioOptions {
            flash_path: opts.flash_a.clone(),
            rpm: current_rpm,
            pedal_pct: opts.pedal_pct,
            gear: opts.gear,
            millis: opts.millis,
        };
        let res_a = run_scenario(&sc_a)?;

        // P (kW) = Torque (Nm) * RPM * 2*pi / 60000
        let p_kw = res_a.delivered_trq_nm * current_rpm * std::f64::consts::PI / 30000.0;
        let p_hp = p_kw * 1.35962; // Metric HP (CV / PS)

        if let Some(ref fb) = opts.flash_b {
            let sc_b = ScenarioOptions {
                flash_path: fb.clone(),
                rpm: current_rpm,
                pedal_pct: opts.pedal_pct,
                gear: opts.gear,
                millis: opts.millis,
            };
            let res_b = run_scenario(&sc_b)?;
            let p_kw_b = res_b.delivered_trq_nm * current_rpm * std::f64::consts::PI / 30000.0;
            let p_hp_b = p_kw_b * 1.35962;

            let diff_trq = res_b.delivered_trq_nm - res_a.delivered_trq_nm;
            let diff_hp = p_hp_b - p_hp;
            let diff_fuel = res_b.fuel_req_mm3 - res_a.fuel_req_mm3;

            writeln!(
                out,
                "{:6.0} | {:7.1} {:+5.1} | {:6.1}kW | {:6.1}HP {:+5.1} | {:6.2} {:+5.2} | {:7.1} | {:6.1}",
                current_rpm,
                res_a.delivered_trq_nm, diff_trq,
                p_kw,
                p_hp, diff_hp,
                res_a.fuel_req_mm3, diff_fuel,
                res_a.boost_desired_kpa,
                res_a.rail_desired_mpa
            )
            .unwrap();
        } else {
            writeln!(
                out,
                "{:6.0} | {:10.1} | {:9.1} | {:9.1} | {:9.2} | {:10.1} | {:10.1}",
                current_rpm,
                res_a.delivered_trq_nm,
                p_kw,
                p_hp,
                res_a.fuel_req_mm3,
                res_a.boost_desired_kpa,
                res_a.rail_desired_mpa
            )
            .unwrap();
        }

        current_rpm += opts.step_rpm;
    }

    Ok(out)
}
