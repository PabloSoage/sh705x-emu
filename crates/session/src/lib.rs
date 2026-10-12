//! The emulated ECU as an interactive front end drives it: the chip wired into its board
//! and car ([`wire`], [`machine`]), the engine's settings ([`Engine`]), the plants around
//! it ([`PlantMode`]), run in slices or a tooth or an edge at a time ([`Session`]), and
//! the history the engine view draws ([`scope`]). The desktop window (`gui`) and the
//! Android app (`android`) both drive it; the command line has its own rig.

mod discover;
pub mod maps;
pub mod names;
pub mod scope;
pub mod sites;

use bridge::link::CYCLES_PER_US;
use engine::crank::Crank;
use engine::plants::{ASTRA, Plants, State};
use engine::sensors::{MAF_KEY, Sensors};
use maps::{MapTrace, Masters};
use sh7059::pins::{Merge, Pin, SquareWave};
use sh7059::{Machine, Sh7059, Stop};
use std::sync::{Arc, Mutex, PoisonError};

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
    /// The maps the firmware reads, while they are traced.
    pub maps: Option<Arc<Mutex<MapTrace>>>,
    /// The breakpoints asked for (the machine's may add the masters, for `map_break`).
    breakpoints: Vec<u32>,
    /// Stop at the next lookup of this map (its descriptor).
    map_break: Option<u32>,
}

/// Passes the masters' breakpoints set for a map break when another map is looked up.
struct MapBreak {
    /// The map stopped for.
    want: u32,
    /// The masters.
    entries: Vec<u32>,
    /// The breakpoints asked for, which always stop.
    user: Vec<u32>,
}

impl MapBreak {
    /// Whether a stop at a breakpoint should go on: at a master, for another map.
    fn passes(&self, m: &Machine) -> bool {
        let pc = m.cpu.pc;
        self.entries.contains(&pc) && !self.user.contains(&pc) && m.cpu.r[4] != self.want
    }
}

/// `m.run_until(n)`, going on past the stops `skip` passes.
fn run_past(m: &mut Machine, n: u64, skip: Option<&MapBreak>) -> Stop {
    let end = m.cpu.instructions + n;
    loop {
        let left = end.saturating_sub(m.cpu.instructions);
        if left == 0 {
            return Stop::Count;
        }
        let s = m.run_until(left);
        if s == Stop::Breakpoint && skip.is_some_and(|k| k.passes(m)) {
            continue;
        }
        return s;
    }
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
            maps: None,
            breakpoints: Vec::new(),
            map_break: None,
        }
    }

    /// Starts tracing the maps the firmware reads (its interpolation routines found in its
    /// flash), or stops.
    pub fn trace_maps(&mut self, on: bool) {
        if on && self.maps.is_none() {
            let chip = &self.m.chip;
            let flash: Vec<u8> = (0..sh7059::ROM_SIZE as u32).map(|a| chip.peek(a)).collect();
            let mut t = MapTrace::attach(&mut self.m, maps::find_masters(&flash));
            t.names = names::Names::builtin(names::calibration(&flash));
            self.maps = Some(Arc::new(Mutex::new(t)));
        } else if !on {
            self.m.probes = None;
            self.maps = None;
            self.map_break = None;
            self.apply_breakpoints();
        }
    }

    /// Names the variables the maps read and give: the sensors, and with `profile` its
    /// parameters (see [`Session::find_names`]); kept with the maps traced, after the
    /// names already there.
    pub fn name_variables(&mut self, profile: Option<&names::Profile>) {
        if self.maps.is_none() {
            return;
        }
        let found = self.find_names(profile);
        if let Some(t) = &self.maps {
            t.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .names
                .merge(found);
        }
    }

    /// Adds map labels (by descriptor) to the maps traced, after those already there.
    pub fn label_maps(&mut self, labels: std::collections::BTreeMap<u32, names::Name>) {
        if let Some(t) = &self.maps {
            t.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .names
                .merge(names::Names {
                    maps: labels,
                    ..Default::default()
                });
        }
    }

    /// The masters being traced.
    pub fn masters(&self) -> Masters {
        self.maps.as_ref().map_or_else(Masters::default, |t| {
            t.lock().unwrap_or_else(PoisonError::into_inner).masters
        })
    }

    /// Replaces the PC breakpoints.
    pub fn set_breakpoints(&mut self, b: Vec<u32>) {
        self.breakpoints = b;
        self.apply_breakpoints();
    }

    /// Stops at the next lookup of the map whose descriptor is `desc` (with the maps
    /// traced), or no longer.
    pub fn break_on_map(&mut self, desc: Option<u32>) {
        self.map_break = desc.filter(|_| self.maps.is_some());
        self.apply_breakpoints();
    }

    /// The map stopped for, if a map break is set.
    pub fn map_break(&self) -> Option<u32> {
        self.map_break
    }

    /// The machine's breakpoints: those asked for, and the masters for a map break.
    fn apply_breakpoints(&mut self) {
        let mut b = self.breakpoints.clone();
        if self.map_break.is_some() {
            let m = self.masters();
            b.extend([m.d3, m.d2].into_iter().flatten());
        }
        self.m.breakpoints = b;
    }

    /// What a map break passes, if one is set.
    fn skip(&self) -> Option<MapBreak> {
        let m = self.masters();
        self.map_break.map(|want| MapBreak {
            want,
            entries: [m.d3, m.d2].into_iter().flatten().collect(),
            user: self.breakpoints.clone(),
        })
    }

    /// Takes the lookups made since the last time into the trace.
    fn take_maps(&mut self) {
        if let Some(t) = &self.maps {
            t.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(&mut self.m);
        }
    }

    /// Executes `n` instructions (breakpoints still stop it), without the plants.
    pub fn step(&mut self, n: u64) -> Stop {
        let skip = self.skip();
        let s = run_past(&mut self.m, n, skip.as_ref());
        self.take_maps();
        s
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
        let skip = self.skip();
        let s = run_past(&mut self.m, cycles, skip.as_ref());
        self.take_maps();
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
        let skip = self.skip();
        let m = &mut self.m;
        let r = m.chip.next_edge().map(|e| {
            let mut s = Stop::Count;
            for _ in 0..STEP_LIMIT {
                if s != Stop::Count || m.chip.next_edge().is_none_or(|n| n.at > e.at) {
                    break;
                }
                s = run_past(m, 1, skip.as_ref());
            }
            s
        });
        self.take_maps();
        r
    }

    /// Runs until the crankshaft has turned to the next tooth position (6°) and its edge
    /// has reached the chip, or a breakpoint; `None` with the engine stopped.
    pub fn next_tooth(&mut self) -> Option<Stop> {
        let skip = self.skip();
        let m = &mut self.m;
        let pitch = |m: &Machine| {
            m.chip
                .crank_angle(m.cpu.cycles)
                .map(|a| (a / 6.0).floor() as u32)
        };
        let r = pitch(m).map(|start| {
            let mut s = Stop::Count;
            for _ in 0..STEP_LIMIT {
                let turned = pitch(m).is_some_and(|p| p != start);
                let due = m.chip.next_edge().is_some_and(|e| e.at <= m.cpu.cycles);
                if s != Stop::Count || (turned && !due) {
                    break;
                }
                s = run_past(m, 1, skip.as_ref());
            }
            s
        });
        self.take_maps();
        r
    }

    /// Adds the moment to `scope` (every moment with `stepping`, else at its pace).
    pub fn sample(&self, scope: &mut scope::Scope, stepping: bool) {
        scope.take(&self.m, self.plant.as_ref(), &self.engine, stepping);
    }
}
