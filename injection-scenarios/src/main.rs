//! Command-line tool for off-idle injection and calibration scenarios.
//!
//! Boots a flash image in the emulator, holds it at a running operating point, and reads
//! back what the firmware asks for: torque, fuel, boost and rail pressure. Point `--flash`
//! at an image you own. Not built into the released binaries; build it yourself (see
//! README.md).

mod dyno;
mod grid;
mod runner;

use dyno::{run_dyno, DynoOptions};
use grid::{run_grid, GridOptions};
use runner::{run_scenario, ScenarioOptions};

const DEFAULT_FLASH: &str = "flash.bin";

fn print_usage() {
    println!(
        "\
Off-idle fuel injection & calibration scenario tool
Usage:
  injection-scenarios scenario [--flash <path>] [--rpm <rpm>] [--pedal <%>] [--gear <1-6>] [--millis <ms>]
  injection-scenarios grid     [--flash <path>] [--compare <path>] [--gear <1-6>]
  injection-scenarios dyno     [--flash <path>] [--compare <path>] [--gear <1-6>] [--start <rpm>] [--end <rpm>]

--flash defaults to ./flash.bin; supply a flash image of a chip you own.

Examples:
  injection-scenarios scenario --flash flash.bin --rpm 2500 --pedal 60 --gear 4
  injection-scenarios grid --flash flash.bin --gear 4
  injection-scenarios dyno --flash flash.bin --start 1400 --end 4200 --gear 4
"
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "scenario" => {
            let mut opts = ScenarioOptions::default();
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--flash" => {
                        opts.flash_path = args.get(i + 1).cloned().unwrap_or(DEFAULT_FLASH.into());
                        i += 2;
                    }
                    "--rpm" => {
                        opts.rpm = args
                            .get(i + 1)
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(2000.0);
                        i += 2;
                    }
                    "--pedal" => {
                        opts.pedal_pct =
                            args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(50.0);
                        i += 2;
                    }
                    "--gear" => {
                        opts.gear = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(4);
                        i += 2;
                    }
                    "--millis" => {
                        opts.millis = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(450);
                        i += 2;
                    }
                    _ => {
                        eprintln!("Unknown scenario option: {}", args[i]);
                        i += 1;
                    }
                }
            }
            let res = run_scenario(&opts)?;
            print_scenario_result(&res);
        }
        "grid" => {
            let mut flash_a = DEFAULT_FLASH.to_string();
            let mut flash_b = None;
            let mut gear = 4;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--flash" => {
                        if let Some(f) = args.get(i + 1) {
                            flash_a = f.clone();
                        }
                        i += 2;
                    }
                    "--compare" => {
                        flash_b = args.get(i + 1).cloned();
                        i += 2;
                    }
                    "--gear" => {
                        gear = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(4);
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            let opts = GridOptions {
                flash_a,
                flash_b,
                rpm_list: vec![750.0, 1500.0, 2000.0, 2500.0, 3000.0, 3800.0],
                pedal_list: vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0],
                gear,
                millis: 450,
            };
            let table = run_grid(&opts)?;
            println!("{table}");
        }
        "dyno" => {
            let mut opts = DynoOptions::default();
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--flash" => {
                        if let Some(f) = args.get(i + 1) {
                            opts.flash_a = f.clone();
                        }
                        i += 2;
                    }
                    "--compare" => {
                        opts.flash_b = args.get(i + 1).cloned();
                        i += 2;
                    }
                    "--gear" => {
                        opts.gear = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(4);
                        i += 2;
                    }
                    "--start" => {
                        opts.start_rpm = args
                            .get(i + 1)
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(1200.0);
                        i += 2;
                    }
                    "--end" => {
                        opts.end_rpm = args
                            .get(i + 1)
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(4200.0);
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            let table = run_dyno(&opts)?;
            println!("{table}");
        }
        _ => {
            print_usage();
        }
    }

    Ok(())
}

fn print_scenario_result(res: &runner::ScenarioResult) {
    println!("=== Scenario Execution Result ===");
    println!(
        "Operating Point:        {:.0} RPM | {:.0}% Pedal | Gear {}",
        res.rpm, res.pedal_pct, res.gear
    );
    println!("Pedal Measured (RAM):   {:.1}%", res.pedal_ram_pct);
    println!(
        "Driver Demand Torque:   {:.1} N·m (Map: 0x09D6EC..7B0)",
        res.driver_demand_nm
    );
    println!("Delivered Torque:       {:.1} N·m", res.delivered_trq_nm);
    println!(
        "Net Output Torque:      {:.1} N·m (DID 0x1A2D)",
        res.net_trq_nm
    );
    println!(
        "Torque Smoke Limiter:   {:.1} N·m (v_FD04)",
        res.trq_limit_smoke_nm
    );
    println!(
        "Fuel Injected / Req:    {:.2} mm3/stroke (v_F8F4)",
        res.fuel_req_mm3
    );
    println!(
        "Fuel Flow Limiter:      {:.2} mm3/stroke (v_0E70)",
        res.flow_limit_mm3
    );
    println!(
        "Desired Boost Pressure: {:.1} kPa ({:.2} bar abs)",
        res.boost_desired_kpa,
        res.boost_desired_kpa / 100.0
    );
    println!(
        "Desired Rail Pressure:  {:.1} MPa ({:.0} bar)",
        res.rail_desired_mpa,
        res.rail_desired_mpa * 10.0
    );
    println!("VGT Vane Duty Cycle:    {:.1}%", res.vgt_duty_pct);
    println!("SCV Pump Current:       {:.0} mA", res.scv_current_ma);
    println!("EGR Valve Position:     {:.1}%", res.egr_position_pct);
    println!("---------------------------------");
    let p_kw = res.delivered_trq_nm * res.rpm * std::f64::consts::PI / 30000.0;
    println!(
        "Calculated Shaft Power: {:.1} kW ({:.1} CV / HP)",
        p_kw,
        p_kw * 1.35962
    );
}
