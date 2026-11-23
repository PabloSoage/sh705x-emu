//! Scenario runner: drive the emulated ECU off idle and read back what the firmware asks
//! for (torque, fuel, boost, rail) at a steady operating point.
//!
//! The emulator has no body-control module in front of the ECU, so the engine-state flags a
//! running car would carry are set directly: "the engine is running, in gear, fuelling
//! enabled". That stands in for a running engine; it is not a bypass of any vehicle security
//! function, and no such function is modelled or touched here.

use engine::plants::{f32_at, Plants, Wiring, ASTRA};
use engine::sensors::{maf_hz, pedal_sensors, Sensors, MAF_KEY};
use engine::wheel::Wheels;
use sh7059::{Machine, Sh7059};

/// RAM variables this tool reads or sets, for one firmware software (SW) id.
///
/// The firmware's RAM layout differs between software releases: reading the AX36 addresses
/// on another release silently returns zeros. The table is chosen from the SW id stored in
/// the flash at 0xC0000; an unknown SW is an error rather than a run of zeros.
#[derive(Clone, Copy, Debug)]
pub struct Addrs {
    /// Master engine running / combustion mode state flag (1 = running).
    pub engine_running_flag: u32,
    /// Global fuel-cut flag (0 = fuelling enabled, 1 = cut).
    pub fuel_enable: u32,
    /// Selected gear (0 = neutral/reverse, 1..6 = gears 1 to 6).
    pub gear_selector: u32,
    /// Driver demand map output (pedal map output, Nm).
    pub driver_demand_trq: u32,
    /// Final delivered torque to the engine (Nm).
    pub delivered_trq: u32,
    /// Net engine torque reported over OBD/GMLAN DID 0x1A2D (Nm); `None` where not located.
    pub net_trq: Option<u32>,
    /// Torque limit from the smoke/flow limiter (Nm).
    pub trq_limit_smoke: u32,
    /// Fuel quantity requested / injected (mm3/stroke).
    pub fuel_requested: u32,
    /// Flow limit (mm3/stroke).
    pub flow_limit: u32,
    /// Desired rail pressure (DID 0x308A, MPa).
    pub rail_pressure_desired: u32,
    /// Desired manifold boost pressure (DID 0x328A, kPa).
    pub boost_pressure_desired: u32,
    /// Accelerator pedal position in RAM (% travel).
    pub pedal_position_ram: u32,
}

/// NCLSW420AX36, the reference image. AX39NCLSW440 shares this RAM layout: the twelve
/// variables have the same number of literal-pool references in both images and the tool
/// returns the same values on both.
pub const AX36: Addrs = Addrs {
    engine_running_flag: 0xFFFE_E6DC,
    fuel_enable: 0xFFFF_29D8,
    gear_selector: 0xFFFE_E100,
    driver_demand_trq: 0xFFFE_FC14,
    delivered_trq: 0xFFFE_FC6C,
    net_trq: Some(0xFFFE_FC4C),
    trq_limit_smoke: 0xFFFE_FD04,
    fuel_requested: 0xFFFE_F8F4,
    flow_limit: 0xFFFF_0E70,
    rail_pressure_desired: 0xFFFE_F924,
    boost_pressure_desired: 0xFFFE_FE84,
    pedal_position_ram: 0xFFFE_D614,
};

/// AZ07NCLNP030. Derived from AX36 by matching the code around every literal-pool load of
/// each variable (the routines are byte-identical; only the literals differ) and taking the
/// literal the same instruction loads here; every variable got a single, unanimous answer.
/// The net-torque variable could not be matched unambiguously and is left out.
pub const AZ07: Addrs = Addrs {
    engine_running_flag: 0xFFFE_E870,
    fuel_enable: 0xFFFF_2B84,
    gear_selector: 0xFFFE_E294,
    driver_demand_trq: 0xFFFE_FDA8,
    delivered_trq: 0xFFFE_FE08,
    net_trq: None,
    trq_limit_smoke: 0xFFFE_FEA0,
    fuel_requested: 0xFFFE_FA88,
    flow_limit: 0xFFFF_100C,
    rail_pressure_desired: 0xFFFE_FAB8,
    boost_pressure_desired: 0xFFFF_0020,
    pedal_position_ram: 0xFFFE_D7A8,
};

/// AZ03NCLNP000, derived the same way as AZ07.
pub const AZ03: Addrs = Addrs {
    engine_running_flag: 0xFFFE_E870,
    fuel_enable: 0xFFFF_2B88,
    gear_selector: 0xFFFE_E294,
    driver_demand_trq: 0xFFFE_FDA8,
    delivered_trq: 0xFFFE_FE0C,
    net_trq: None,
    trq_limit_smoke: 0xFFFE_FEA4,
    fuel_requested: 0xFFFE_FA88,
    flow_limit: 0xFFFF_1010,
    rail_pressure_desired: 0xFFFE_FAB8,
    boost_pressure_desired: 0xFFFF_0024,
    pedal_position_ram: 0xFFFE_D7A8,
};

/// The 12-character software id at 0xC0000 (e.g. "AX36NCLSW420").
pub fn sw_id(flash: &[u8]) -> Option<String> {
    let id = flash.get(0xC0000..0xC000C)?;
    id.iter()
        .all(|b| b.is_ascii_alphanumeric())
        .then(|| String::from_utf8_lossy(id).into_owned())
}

/// The RAM table for a flash image, or an error naming the SW and the supported ones.
pub fn addrs_for(flash: &[u8]) -> Result<Addrs, String> {
    let id = sw_id(flash).unwrap_or_else(|| "?".into());
    match id.as_str() {
        "AX36NCLSW420" | "AX39NCLSW440" => Ok(AX36),
        "AZ07NCLNP030" => Ok(AZ07),
        "AZ03NCLNP000" => Ok(AZ03),
        _ => Err(format!(
            "software '{id}' is not supported: its RAM addresses are unknown and reading the \
             AX36 ones would return zeros (supported: AX36NCLSW420, AX39NCLSW440, AZ07NCLNP030, \
             AZ03NCLNP000)"
        )),
    }
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
    let addrs = addrs_for(&bytes)?;
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
    let wiring = Wiring {
        quantity_at: addrs.fuel_requested,
        ..ASTRA
    };
    let mut plants = Plants::new(wiring, opts.rpm, None, 100.0, 30.0);

    let mut last_vgt = 0.0;
    let mut last_scv_ma = 0.0;
    let mut last_egr = 0.0;

    for _ in 0..opts.millis {
        // Engine state: running, fuelling enabled, in gear. No BCM in the emulator, so these
        // are set directly rather than arriving from one. See the module docs.
        m.chip.poke_ram(addrs.engine_running_flag, 0x01);
        m.chip.poke_ram(addrs.fuel_enable, 0x00);
        m.chip.poke_ram(addrs.gear_selector, opts.gear);

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
        pedal_ram_pct: f32_at(&m, addrs.pedal_position_ram),
        driver_demand_nm: f32_at(&m, addrs.driver_demand_trq),
        delivered_trq_nm: f32_at(&m, addrs.delivered_trq),
        net_trq_nm: addrs.net_trq.map_or(f64::NAN, |a| f32_at(&m, a)),
        trq_limit_smoke_nm: f32_at(&m, addrs.trq_limit_smoke),
        fuel_req_mm3: f32_at(&m, addrs.fuel_requested),
        flow_limit_mm3: f32_at(&m, addrs.flow_limit),
        boost_desired_kpa: f32_at(&m, addrs.boost_pressure_desired),
        rail_desired_mpa: f32_at(&m, addrs.rail_pressure_desired),
        vgt_duty_pct: last_vgt,
        scv_current_ma: last_scv_ma,
        egr_position_pct: last_egr,
    })
}
