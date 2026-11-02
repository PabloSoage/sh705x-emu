//! Scenario runner: drive the emulated ECU off idle and read back what the firmware asks
//! for (torque, fuel, boost, rail) at a steady operating point.
//!
//! The emulator has no body-control module in front of the ECU, so the engine-state flags a
//! running car would carry are set directly: "the engine is running, in gear, fuelling
//! enabled". That stands in for a running engine; it is not a bypass of any vehicle security
//! function, and no such function is modelled or touched here.

use engine::plants::{f32_at, Plants, ASTRA};
use engine::sensors::{maf_hz, pedal_sensors, Sensors, MAF_KEY};
use engine::wheel::Wheels;
use sh7059::{Machine, Sh7059};

/// RAM variables of the Astra J SH7059 firmware (NCLSW420AX36) this tool reads or sets.
#[allow(dead_code)]
pub mod addrs {
    // --- Engine state set by this tool (emulator-side; see the module docs) ---
    /// Master engine running / combustion mode state flag (1 = running).
    pub const ENGINE_RUNNING_FLAG: u32 = 0xFFFE_E6DC;
    /// Global fuel-cut flag (0 = fuelling enabled, 1 = cut).
    pub const FUEL_ENABLE: u32 = 0xFFFF_29D8;
    /// Selected gear (0 = neutral/reverse, 1..6 = gears 1 to 6).
    pub const GEAR_SELECTOR: u32 = 0xFFFE_E100;

    // --- Values read back after the run ---
    /// Driver demand map output (pedal map output, Nm).
    pub const DRIVER_DEMAND_TRQ: u32 = 0xFFFE_FC14;
    /// Final delivered torque to the engine (v_FC6C, Nm).
    pub const DELIVERED_TRQ: u32 = 0xFFFE_FC6C;
    /// Net engine torque reported over OBD/GMLAN DID 0x1A2D (v_FC4C, Nm).
    pub const NET_TRQ: u32 = 0xFFFE_FC4C;
    /// Torque limit from the smoke/flow limiter (v_FD04, Nm).
    pub const TRQ_LIMIT_SMOKE: u32 = 0xFFFE_FD04;
    /// Fuel quantity requested / injected (v_F8F4, mm3/stroke).
    pub const FUEL_REQUESTED: u32 = 0xFFFE_F8F4;
    /// Flow limit (v_0E70, mm3/stroke).
    pub const FLOW_LIMIT_0E70: u32 = 0xFFFF_0E70;
    /// Desired rail pressure (v_F924 / DID 0x308A, MPa).
    pub const RAIL_PRESSURE_DESIRED: u32 = 0xFFFE_F924;
    /// Desired manifold boost pressure (v_FE84 / DID 0x328A, kPa).
    pub const BOOST_PRESSURE_DESIRED: u32 = 0xFFFE_FE84;
    /// Accelerator pedal position in RAM (v_D614, % travel).
    pub const PEDAL_POSITION_RAM: u32 = 0xFFFE_D614;
}

/// Scenario output readings at steady state.
#[derive(Clone, Debug, Default)]
pub struct ScenarioResult {
    pub rpm: f64,
    pub pedal_pct: f64,
    pub gear: u8,
    pub pedal_ram_pct: f64,
    pub driver_demand_nm: f64,
    pub delivered_trq_nm: f64,
    pub net_trq_nm: f64,
    pub trq_limit_smoke_nm: f64,
    pub fuel_req_mm3: f64,
    pub flow_limit_mm3: f64,
    pub boost_desired_kpa: f64,
    pub rail_desired_mpa: f64,
    pub vgt_duty_pct: f64,
    pub scv_current_ma: f64,
    pub egr_position_pct: f64,
}

/// Options for configuring an ECU scenario run.
#[derive(Clone, Debug)]
pub struct ScenarioOptions {
    pub flash_path: String,
    pub rpm: f64,
    pub pedal_pct: f64,
    pub gear: u8,
    pub millis: u64,
}

impl Default for ScenarioOptions {
    fn default() -> Self {
        Self {
            flash_path: "flash.bin".into(),
            rpm: 2000.0,
            pedal_pct: 50.0,
            gear: 4, // 4th gear default for dyno / road load
            millis: 400,
        }
    }
}

/// Builds, boots, and runs the simulated ECU scenario off idle.
pub fn run_scenario(opts: &ScenarioOptions) -> Result<ScenarioResult, String> {
    let bytes = std::fs::read(&opts.flash_path)
        .map_err(|e| format!("Failed to read flash file '{}': {e}", opts.flash_path))?;
    let mut chip =
        Sh7059::new(&bytes).map_err(|e| format!("Failed to initialize SH7059 core: {e}"))?;
    board::wire_status(&mut chip);

    let mut s = Sensors::idle();
    let (s1, s2) = pedal_sensors(opts.pedal_pct);
    s.set("pedal1", s1)?;
    s.set("pedal2", s2)?;

    for (c, v) in s.codes() {
        chip.adc.input[c] = v;
    }

    let mut pins = sh7059::pins::Merge::default();
    pins.0.push(Box::new(Wheels::new(opts.rpm, 0)));
    // Estimate initial MAF from RPM and pedal to prime the smoke limiter.
    let est_maf = (opts.rpm / 1000.0) * (18.0 + 0.65 * opts.pedal_pct);
    pins.0.push(Box::new(sh7059::pins::SquareWave::new(
        sh7059::pins::Pin::Ti0(2),
        MAF_KEY,
        maf_hz(est_maf),
        engine::wheel::CYCLES_PER_S,
        0,
    )));
    chip.set_stimulus(Box::new(pins));

    let mut m = Machine::new(chip, true);
    let mut plants = Plants::new(ASTRA, opts.rpm, None, 100.0, 30.0);

    let mut last_vgt = 0.0;
    let mut last_scv_ma = 0.0;
    let mut last_egr = 0.0;

    for _ in 0..opts.millis {
        // Engine state: running, fuelling enabled, in gear. No BCM in the emulator, so these
        // are set directly rather than arriving from one. See the module docs.
        m.chip.poke_ram(addrs::ENGINE_RUNNING_FLAG, 0x01);
        m.chip.poke_ram(addrs::FUEL_ENABLE, 0x00);
        m.chip.poke_ram(addrs::GEAR_SELECTOR, opts.gear);

        m.run(1000 * bridge::link::CYCLES_PER_US);
        m.chip.hcan.ch[0].outbox.clear();
        let st = plants.step(&mut m, 0.001);

        last_vgt = st.vgt_duty.unwrap_or(0.0) * 100.0;
        last_scv_ma = st.scv_ma;
        last_egr = st.egr;
    }

    Ok(ScenarioResult {
        rpm: opts.rpm,
        pedal_pct: opts.pedal_pct,
        gear: opts.gear,
        pedal_ram_pct: f32_at(&m, addrs::PEDAL_POSITION_RAM),
        driver_demand_nm: f32_at(&m, addrs::DRIVER_DEMAND_TRQ),
        delivered_trq_nm: f32_at(&m, addrs::DELIVERED_TRQ),
        net_trq_nm: f32_at(&m, addrs::NET_TRQ),
        trq_limit_smoke_nm: f32_at(&m, addrs::TRQ_LIMIT_SMOKE),
        fuel_req_mm3: f32_at(&m, addrs::FUEL_REQUESTED),
        flow_limit_mm3: f32_at(&m, addrs::FLOW_LIMIT_0E70),
        boost_desired_kpa: f32_at(&m, addrs::BOOST_PRESSURE_DESIRED),
        rail_desired_mpa: f32_at(&m, addrs::RAIL_PRESSURE_DESIRED),
        vgt_duty_pct: last_vgt,
        scv_current_ma: last_scv_ma,
        egr_position_pct: last_egr,
    })
}
