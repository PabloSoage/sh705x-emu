//! The emulated ECU as an interactive front end drives it: the chip wired into its board
//! and car ([`wire`], [`machine`]), the engine's settings ([`Engine`]), the plants around
//! it ([`PlantMode`]), run in slices or a tooth or an edge at a time ([`Session`]), and
//! the history the engine view draws ([`scope`]). The desktop window (`gui`) and the
//! Android app (`android`) both drive it; the command line has its own rig.

pub mod scope;

use bridge::link::CYCLES_PER_US;
use engine::crank::Crank;
use engine::plants::{ASTRA, Plants, State};
use engine::sensors::{MAF_KEY, Sensors};
use sh7059::pins::{Merge, Pin, SquareWave};
use sh7059::{Machine, Sh7059, Stop};

/// AN14 code that turns the ignition on (see `sh705x` IGNITION_ON).
pub const IGNITION_ON: u16 = 600;

/// The board and the car around the chip: status chips, ignition on, the crank and cam
/// wheels (stopped) and the MAF, so both can be set while running, and the ABS on the bus.
pub fn wire(chip: &mut Sh7059) {
    board::wire_status(chip);
    chip.adc.input[14] = IGNITION_ON;
    chip.set_stimulus(Box::new(Merge(vec![
        Box::new(engine::wheel::Wheels::new(0.0, 0)),
        Box::new(SquareWave::new(
            Pin::Ti0(2),
            engine::sensors::MAF_KEY,
            0.0,
            engine::wheel::CYCLES_PER_S,
            0,
        )),
    ])));
    chip.set_can_source(Box::new(engine::bus::abs()));
}

/// A machine on `flash`, with `eeprom` (the board's four 93C86, 8192 bytes, as `layout`
/// places them) if given, [`wire`]d, at reset.
pub fn machine(
    flash: &[u8],
    eeprom: Option<&[u8]>,
    layout: board::Layout,
) -> Result<Machine, String> {
    let mut chip = Sh7059::new(flash).map_err(|e| e.to_string())?;
    if let Some(bytes) = eeprom {
        let e = board::Eeprom::new(bytes).map_err(|e| e.to_string())?;
        if e.len() != 4 * board::CHIP_BYTES {
            return Err("the board has four 93C86, 8192 bytes".into());
        }
        board::wire_eeproms(&mut chip, &e, layout);
    }
    wire(&mut chip);
    Ok(Machine::new(chip, true))
}

/// The simulated engine's settings: what drives the ECU's inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct Engine {
    /// Speed, rpm; 0 is stopped.
    pub rpm: f64,
    /// Sensors in physical units.
    pub sensors: Sensors,
}

impl Engine {
    /// A warm engine with the ignition on, not turning.
    pub fn ignition_on() -> Self {
        Self {
            rpm: 0.0,
            sensors: Sensors::ignition_on(),
        }
    }

    /// The car's warm idle: 750 rpm, the sensors as it reports them.
    pub fn idle() -> Self {
        Self {
            rpm: 750.0,
            sensors: Sensors::idle(),
        }
    }

    /// A sensor's value as set, if `key` is one.
    pub fn sensor(&self, key: &str) -> Option<f64> {
        self.sensors
            .values
            .iter()
            .find(|(x, _)| *x == key)
            .map(|&(_, v)| v)
    }

    /// Puts every setting on the machine: A/D codes, the wheels' speed, the MAF.
    pub fn apply(&self, m: &mut Machine) {
        for (c, v) in self.sensors.codes() {
            m.chip.adc.input[c] = v;
        }
        m.chip.stimulus_set("rpm", self.rpm);
        if let Some(hz) = self.sensors.maf_hz() {
            m.chip.stimulus_set(MAF_KEY, hz);
        }
    }
}

/// The plants around the ECU (`engine::plants`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlantMode {
    /// None: the inputs are what the sensors are set to.
    Off,
    /// The SCV, EGR, fresh air and turbocharger close their loops; the speed is what it
    /// is set to.
    Held,
    /// And the engine turns on the firmware's torque, against the idling car's load.
    Turning,
}

/// The idling car's load beyond the friction the firmware models, N·m (see `sh705x plant`).
const IDLE_LOAD: f64 = 12.0;
/// The plants' step, CPU cycles: a millisecond.
const PLANT_CYCLES: u64 = 1000 * CYCLES_PER_US;
/// ECU time the crankshaft is held at its speed after power-on, cycles: half a second, the
/// command line's default boot.
const BOOT_CYCLES: u64 = 500_000 * CYCLES_PER_US;
/// Instructions a tooth or edge step runs at most before giving up: a tooth is about
/// 100 000 at 750 rpm.
const STEP_LIMIT: usize = 50_000_000;

/// The machine, the engine's settings and the plants, run together.
pub struct Session {
    /// The machine.
    pub m: Machine,
    /// The engine as set: speed and sensors.
    pub engine: Engine,
    /// The plants around the ECU.
    pub plant_mode: PlantMode,
    /// What they gave last, if they are on.
    pub plant: Option<State>,
    plants: Option<Plants>,
    /// Cycle of the plants' last step.
    plant_cycle: u64,
}

impl Session {
    /// `m` as it is, with `engine` as its settings (put on its inputs or not: see
    /// [`Engine::apply`]), no plants.
    pub fn new(m: Machine, engine: Engine) -> Self {
        let plant_cycle = m.cpu.cycles;
        Self {
            m,
            engine,
            plant_mode: PlantMode::Off,
            plant: None,
            plants: None,
            plant_cycle,
        }
    }

    /// Sets the engine speed, rpm (0 stops it); with the engine turning on its torque,
    /// that is where the crankshaft goes on from.
    pub fn set_rpm(&mut self, rpm: f64) {
        self.engine.rpm = rpm.max(0.0);
        self.engine.apply(&mut self.m);
        if let Some(p) = self.plants.as_mut() {
            p.rpm = self.engine.rpm;
            if let Some(c) = p.crank.as_mut() {
                c.rpm = self.engine.rpm;
            }
        }
    }

    /// Sets a sensor by key, in physical units.
    pub fn set_sensor(&mut self, key: &str, value: f64) -> Result<(), String> {
        self.engine.sensors.set(key, value)?;
        self.engine.apply(&mut self.m);
        Ok(())
    }

    /// All sensors and the speed at once: ignition on and still, or idling.
    pub fn preset(&mut self, idle: bool) {
        self.engine = if idle {
            Engine::idle()
        } else {
            Engine::ignition_on()
        };
        self.engine.apply(&mut self.m);
    }

    /// Puts the plants around the ECU, or takes them away.
    pub fn set_plants(&mut self, mode: PlantMode) {
        self.plant_mode = mode;
        let rest = self.engine.sensor("boost").unwrap_or(98.0);
        let intake = self.engine.sensor("intake2").unwrap_or(26.0);
        let rpm = self.engine.rpm;
        self.plants = match mode {
            PlantMode::Off => None,
            PlantMode::Held => Some(Plants::new(ASTRA, rpm, None, rest, intake)),
            PlantMode::Turning => {
                let mut c = Crank::new(rpm);
                c.load = IDLE_LOAD;
                Some(Plants::new(ASTRA, rpm, Some(c), rest, intake))
            }
        };
        self.plant = None;
        self.plant_cycle = self.m.cpu.cycles;
        if self.plants.is_none() {
            self.engine.apply(&mut self.m);
        }
    }

    /// The plants pick up from now: after the machine ran without them (a step, a
    /// diagnostic exchange) or after a pause.
    pub fn resync(&mut self) {
        self.plant_cycle = self.m.cpu.cycles;
    }

    /// Runs `cycles` (breakpoints still stop it), then steps the plants if a millisecond
    /// of ECU time has gone by since their last step.
    pub fn run(&mut self, cycles: u64) -> Stop {
        let s = self.m.run_until(cycles);
        if let Some(p) = self.plants.as_mut()
            && self.m.cpu.cycles - self.plant_cycle >= PLANT_CYCLES
        {
            let dt = (self.m.cpu.cycles - self.plant_cycle) as f64 / (CYCLES_PER_US as f64 * 1e6);
            // The crankshaft turns once the firmware has booted (see sh705x rig).
            p.held = self.m.cpu.cycles < BOOT_CYCLES;
            let st = p.step(&mut self.m, dt);
            self.engine.rpm = st.rpm;
            self.plant = Some(st);
            self.plant_cycle = self.m.cpu.cycles;
        }
        s
    }

    /// Runs until the next edge on the input pins (crank, cam or MAF) has reached the
    /// chip, or a breakpoint; `None` if no edge is coming.
    pub fn next_edge(&mut self) -> Option<Stop> {
        let m = &mut self.m;
        m.chip.next_edge().map(|e| {
            let mut s = Stop::Count;
            for _ in 0..STEP_LIMIT {
                if s != Stop::Count || m.chip.next_edge().is_none_or(|n| n.at > e.at) {
                    break;
                }
                s = m.run_until(1);
            }
            s
        })
    }

    /// Runs until the crankshaft has turned to the next tooth position (6°) and its edge
    /// has reached the chip, or a breakpoint; `None` with the engine stopped.
    pub fn next_tooth(&mut self) -> Option<Stop> {
        let m = &mut self.m;
        let pitch = |m: &Machine| {
            m.chip
                .crank_angle(m.cpu.cycles)
                .map(|a| (a / 6.0).floor() as u32)
        };
        pitch(m).map(|start| {
            let mut s = Stop::Count;
            for _ in 0..STEP_LIMIT {
                let turned = pitch(m).is_some_and(|p| p != start);
                let due = m.chip.next_edge().is_some_and(|e| e.at <= m.cpu.cycles);
                if s != Stop::Count || (turned && !due) {
                    break;
                }
                s = m.run_until(1);
            }
            s
        })
    }

    /// Adds the moment to `scope` (every moment with `stepping`, else at its pace).
    pub fn sample(&self, scope: &mut scope::Scope, stepping: bool) {
        scope.take(&self.m, self.plant.as_ref(), &self.engine, stepping);
    }
}
