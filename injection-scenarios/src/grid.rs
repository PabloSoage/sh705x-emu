//! 2D Operating Grid Evaluation and Calibration Comparison.

use crate::runner::{run_scenario, ScenarioOptions, ScenarioResult};
use std::fmt::Write as _;

pub struct GridOptions {
    pub flash_a: String,
    pub flash_b: Option<String>,
    pub rpm_list: Vec<f64>,
    pub pedal_list: Vec<f64>,
    pub gear: u8,
    pub millis: u64,
}

pub fn run_grid(opts: &GridOptions) -> Result<String, String> {
    let mut out = String::new();

    writeln!(
        out,
        "=== Calibration Operating Grid (Gear {}) ===",
        opts.gear
    )
    .unwrap();
    writeln!(out, "Flash A: {}", opts.flash_a).unwrap();
    if let Some(ref fb) = opts.flash_b {
        writeln!(out, "Flash B (Compare): {}", fb).unwrap();
    }
    writeln!(out).unwrap();

    let mut results_a: Vec<Vec<ScenarioResult>> = Vec::new();
    let mut results_b: Vec<Vec<ScenarioResult>> = Vec::new();

    for &rpm in &opts.rpm_list {
        let mut row_a = Vec::new();
        let mut row_b = Vec::new();
        for &pedal in &opts.pedal_list {
            let sc_a = ScenarioOptions {
                flash_path: opts.flash_a.clone(),
                rpm,
                pedal_pct: pedal,
                gear: opts.gear,
                millis: opts.millis,
            };
            let res_a = run_scenario(&sc_a)?;
            row_a.push(res_a);

            if let Some(ref fb) = opts.flash_b {
                let sc_b = ScenarioOptions {
                    flash_path: fb.clone(),
                    rpm,
                    pedal_pct: pedal,
                    gear: opts.gear,
                    millis: opts.millis,
                };
                let res_b = run_scenario(&sc_b)?;
                row_b.push(res_b);
            }
        }
        results_a.push(row_a);
        if opts.flash_b.is_some() {
            results_b.push(row_b);
        }
    }

    let gb_ref = if opts.flash_b.is_some() {
        Some(results_b.as_slice())
    } else {
        None
    };

    // Print Delivered Torque Grid
    print_matrix(
        &mut out,
        "DELIVERED TORQUE (Nm)",
        &opts.rpm_list,
        &opts.pedal_list,
        &results_a,
        gb_ref,
        |r| r.delivered_trq_nm,
    );

    // Print Fuel Requested Grid
    print_matrix(
        &mut out,
        "FUEL REQUESTED (mm3/stroke)",
        &opts.rpm_list,
        &opts.pedal_list,
        &results_a,
        gb_ref,
        |r| r.fuel_req_mm3,
    );

    // Print Desired Boost Pressure Grid
    print_matrix(
        &mut out,
        "DESIRED BOOST PRESSURE (kPa)",
        &opts.rpm_list,
        &opts.pedal_list,
        &results_a,
        gb_ref,
        |r| r.boost_desired_kpa,
    );

    // Print Desired Rail Pressure Grid
    print_matrix(
        &mut out,
        "DESIRED RAIL PRESSURE (MPa)",
        &opts.rpm_list,
        &opts.pedal_list,
        &results_a,
        gb_ref,
        |r| r.rail_desired_mpa,
    );

    Ok(out)
}

fn print_matrix<F>(
    out: &mut String,
    title: &str,
    rpm_list: &[f64],
    pedal_list: &[f64],
    grid_a: &[Vec<ScenarioResult>],
    grid_b: Option<&[Vec<ScenarioResult>]>,
    field: F,
) where
    F: Fn(&ScenarioResult) -> f64,
{
    writeln!(out, "--- {title} ---").unwrap();
    write!(out, "{:>10}", "rpm \\ pedal").unwrap();
    for &p in pedal_list {
        write!(out, "{:>10}", format!("{p:.0}%")).unwrap();
    }
    writeln!(out).unwrap();

    for (r, &rpm) in rpm_list.iter().enumerate() {
        write!(out, "{:>10.0}", rpm).unwrap();
        for (c, _) in pedal_list.iter().enumerate() {
            let val_a = field(&grid_a[r][c]);
            if let Some(gb) = grid_b {
                let val_b = field(&gb[r][c]);
                let diff = val_b - val_a;
                if diff.abs() > 0.05 {
                    write!(out, "{:>10}", format!("{val_a:.1}|{diff:+.1}")).unwrap();
                } else {
                    write!(out, "{:>10.1}", val_a).unwrap();
                }
            } else {
                write!(out, "{:>10.1}", val_a).unwrap();
            }
        }
        writeln!(out).unwrap();
    }
    writeln!(out).unwrap();
}
