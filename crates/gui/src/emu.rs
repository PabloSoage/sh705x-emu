//! The emulation thread: it owns the machine, takes orders from the interface over a
//! channel, and publishes a snapshot about every 30 ms (ADR-0004).

use bridge::elm::Elm327;
use bridge::link::{CYCLES_PER_US, VirtualLink};
use engine::crank::Crank;
use engine::plants::{ASTRA, Plants, State};
use engine::sensors::{MAF_KEY, Sensors};
use sh2e::Cpu;
use sh7059::hcan::Frame;
use sh7059::{Machine, Stop};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Orders from the interface.
#[derive(Clone, Debug)]
pub enum Cmd {
    /// Run until paused or stopped.
    Run,
    /// Pause.
    Pause,
    /// Execute this many instructions (breakpoints still stop it).
    Step(u64),
    /// Replace the PC breakpoints.
    Breakpoints(Vec<u32>),
    /// Set an A/D input (channel, 10-bit code).
    Adc(usize, u16),
    /// Set a sensor by key, in physical units.
    Sensor(&'static str, f64),
    /// Set the engine speed, rpm (0 stops it).
    Rpm(f64),
    /// All sensors and the speed at once: ignition on and still (false) or idling.
    Preset(bool),
    /// Show memory from this address.
    Memory(u32),
    /// Paused, run until the next edge on the input pins (crank, cam or MAF) has
    /// reached the chip; breakpoints still stop it.
    NextEdge,
    /// Paused, run until the crankshaft has turned to the next tooth position (6°) and
    /// its edge has reached the chip; breakpoints still stop it.
    NextTooth,
    /// Follow the wall clock (true) or run flat out.
    RealTime(bool),
    /// ECU time per wall-clock time when following the wall clock: 1 is real time,
    /// 0.001 shows a tooth at idle about every second.
    Speed(f64),
    /// Send an ELM327 command line over the emulated CAN bus, in virtual time.
    Elm(String),
    /// Put the plants around the ECU, or take them away.
    Plants(PlantMode),
    /// End the thread.
    Quit,
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

/// Who put a frame on the bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The ECU.
    Ecu,
    /// The diagnostic tool (the ELM327 console).
    Tool,
    /// Another node of the simulated car (the ABS).
    Node,
}

/// One CAN frame seen on HCAN0: when, who sent it, what.
#[derive(Clone, Copy, Debug)]
pub struct CanEvent {
    /// Emulated time, µs.
    pub us: u64,
    /// Who put it on the bus.
    pub from: Origin,
    /// The frame.
    pub frame: Frame,
}

/// What the interface shows, as of the last publication.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// The CPU.
    pub cpu: Cpu,
    /// Running or paused.
    pub running: bool,
    /// Why it last stopped.
    pub stop: Option<Stop>,
    /// Interrupts taken, per vector, for the vectors that have any.
    pub interrupts: Vec<(usize, u64)>,
    /// The latest CAN frames, oldest first.
    pub can: VecDeque<CanEvent>,
    /// Address of the memory window.
    pub mem_addr: u32,
    /// The memory window.
    pub mem: Vec<u8>,
    /// Emulated time over wall time over the last second.
    pub rt_factor: f64,
    /// A/D inputs.
    pub adc: [u16; 32],
    /// ECU time per wall-clock time when following it.
    pub speed: f64,
    /// The crankshaft angle, degrees over 720, if the engine turns.
    pub crank_angle: Option<f64>,
    /// The next edge on the input pins.
    pub next_edge: Option<sh7059::pins::Edge>,
    /// Following the wall clock.
    pub real_time: bool,
    /// The ELM327 console: commands and answers.
    pub console: Vec<String>,
    /// Bytes sent and received on each SCI channel.
    pub sci: [(usize, usize); 5],
    /// The engine as set: speed and sensors.
    pub engine: Engine,
    /// The plants around the ECU.
    pub plant_mode: PlantMode,
    /// What they gave last, if they are on.
    pub plant: Option<State>,
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

/// The idling car's load beyond the friction the firmware models, N·m (see `sh705x plant`).
const IDLE_LOAD: f64 = 12.0;
/// The plants' step, CPU cycles: a millisecond.
const PLANT_CYCLES: u64 = 1000 * CYCLES_PER_US;
/// ECU time the crankshaft is held at its speed after power-on, cycles: half a second, the
/// command line's default boot.
const BOOT_CYCLES: u64 = 500_000 * CYCLES_PER_US;

/// Frames kept for the CAN view.
const CAN_KEPT: usize = 400;
/// Bytes in the memory window.
pub const MEM_WINDOW: u32 = 256;

/// Runs the machine until told to quit.
pub fn run(mut m: Machine, mut engine: Engine, cmds: Receiver<Cmd>, shared: Arc<Mutex<Snapshot>>) {
    let mut running = false;
    let mut real_time = true;
    let mut speed = 1.0f64;
    let mut stop = None;
    let mut mem_addr = sh7059::RAM_BASE;
    let mut can: VecDeque<CanEvent> = VecDeque::new();
    let mut console: Vec<String> = Vec::new();
    let mut elm = Elm327::new();
    elm.echo = false;
    // Pacing: the cycle and wall-clock instant real time is measured from.
    let (mut pace_cycle, mut pace_wall) = (m.cpu.cycles, Instant::now());
    // Real-time factor over the last second.
    let (mut rt_cycle, mut rt_wall, mut rt_factor) = (m.cpu.cycles, Instant::now(), 0.0);
    let mut published = Instant::now() - Duration::from_secs(1);
    let mut plant_mode = PlantMode::Off;
    let mut plants: Option<Plants> = None;
    let mut plant: Option<State> = None;
    let mut plant_cycle = m.cpu.cycles;
    loop {
        // Orders: wait for one while paused, take what is there while running.
        let first = if running {
            cmds.try_recv().ok()
        } else {
            match cmds.recv_timeout(Duration::from_millis(30)) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };
        let mut next = first;
        while let Some(c) = next.take() {
            match c {
                Cmd::Run => {
                    running = true;
                    stop = None;
                    plant_cycle = m.cpu.cycles;
                    (pace_cycle, pace_wall) = (m.cpu.cycles, Instant::now());
                }
                Cmd::Pause => running = false,
                Cmd::Step(n) => {
                    running = false;
                    stop = Some(m.run_until(n));
                }
                Cmd::Breakpoints(b) => m.breakpoints = b,
                Cmd::Adc(c, v) => m.chip.adc.input[c] = v,
                Cmd::Sensor(k, v) => {
                    if engine.sensors.set(k, v).is_ok() {
                        engine.apply(&mut m);
                    }
                }
                Cmd::Rpm(r) => {
                    engine.rpm = r.max(0.0);
                    engine.apply(&mut m);
                    if let Some(p) = plants.as_mut() {
                        p.rpm = engine.rpm;
                        if let Some(c) = p.crank.as_mut() {
                            c.rpm = engine.rpm;
                        }
                    }
                }
                Cmd::Plants(mode) => {
                    plant_mode = mode;
                    let get = |k: &str| {
                        engine
                            .sensors
                            .values
                            .iter()
                            .find(|(x, _)| *x == k)
                            .map(|&(_, v)| v)
                    };
                    let (rest, intake) =
                        (get("boost").unwrap_or(98.0), get("intake2").unwrap_or(26.0));
                    plants = match mode {
                        PlantMode::Off => None,
                        PlantMode::Held => Some(Plants::new(ASTRA, engine.rpm, None, rest, intake)),
                        PlantMode::Turning => {
                            let mut c = Crank::new(engine.rpm);
                            c.load = IDLE_LOAD;
                            Some(Plants::new(ASTRA, engine.rpm, Some(c), rest, intake))
                        }
                    };
                    plant = None;
                    plant_cycle = m.cpu.cycles;
                    if plants.is_none() {
                        engine.apply(&mut m);
                    }
                }
                Cmd::Preset(idle) => {
                    engine = if idle {
                        Engine {
                            rpm: 750.0,
                            sensors: Sensors::idle(),
                        }
                    } else {
                        Engine::ignition_on()
                    };
                    engine.apply(&mut m);
                }
                Cmd::Memory(a) => mem_addr = a,
                Cmd::RealTime(r) => {
                    real_time = r;
                    (pace_cycle, pace_wall) = (m.cpu.cycles, Instant::now());
                }
                Cmd::Speed(s) => {
                    speed = s.clamp(1e-4, 1.0);
                    (pace_cycle, pace_wall) = (m.cpu.cycles, Instant::now());
                }
                Cmd::NextEdge => {
                    running = false;
                    stop = m.chip.next_edge().map(|e| {
                        // A tooth is about 100 000 instructions at 750 rpm; stop well past
                        // that rather than run on if the edge never comes.
                        let mut s = Stop::Count;
                        for _ in 0..50_000_000 {
                            if s != Stop::Count || m.chip.next_edge().is_none_or(|n| n.at > e.at) {
                                break;
                            }
                            s = m.run_until(1);
                        }
                        s
                    });
                }
                Cmd::NextTooth => {
                    running = false;
                    let pitch = |m: &Machine| {
                        m.chip
                            .crank_angle(m.cpu.cycles)
                            .map(|a| (a / 6.0).floor() as u32)
                    };
                    stop = pitch(&m).map(|start| {
                        let mut s = Stop::Count;
                        for _ in 0..50_000_000 {
                            let turned = pitch(&m).is_some_and(|p| p != start);
                            let due = m.chip.next_edge().is_some_and(|e| e.at <= m.cpu.cycles);
                            if s != Stop::Count || (turned && !due) {
                                break;
                            }
                            s = m.run_until(1);
                        }
                        s
                    });
                }
                Cmd::Elm(line) => {
                    let us = m.cpu.cycles / CYCLES_PER_US;
                    let answer = {
                        let mut link = LoggingLink {
                            inner: VirtualLink::new(&mut m, 0),
                            can: &mut can,
                        };
                        elm.line(&line, &mut link)
                    };
                    console.push(format!("[{:.3} s] > {line}", us as f64 / 1e6));
                    for l in answer.trim_end_matches('>').split('\r') {
                        if !l.trim().is_empty() {
                            console.push(format!("    {}", l.trim_end()));
                        }
                    }
                    (pace_cycle, pace_wall) = (m.cpu.cycles, Instant::now());
                    // The plants did not follow the exchange: pick up from here.
                    plant_cycle = m.cpu.cycles;
                }
                Cmd::Quit => return,
            }
            next = cmds.try_recv().ok();
        }
        if running {
            let ahead = real_time && {
                let wall_us = pace_wall.elapsed().as_micros() as u64;
                (m.cpu.cycles - pace_cycle) as f64 > wall_us as f64 * CYCLES_PER_US as f64 * speed
            };
            if ahead {
                std::thread::sleep(Duration::from_micros(500));
            } else {
                // Slowed down, in smaller slices, so the teeth come one by one.
                let slice = if real_time {
                    (40_000.0 * speed).max(200.0) as u64
                } else {
                    40_000
                };
                let s = m.run_until(slice);
                if s != Stop::Count {
                    running = false;
                    stop = Some(s);
                }
                if let Some(p) = plants.as_mut() {
                    // A step a millisecond at most, as on the command line.
                    if m.cpu.cycles - plant_cycle >= PLANT_CYCLES {
                        let dt = (m.cpu.cycles - plant_cycle) as f64 / (CYCLES_PER_US as f64 * 1e6);
                        // The crankshaft turns once the firmware has booted (see sh705x rig).
                        p.held = m.cpu.cycles < BOOT_CYCLES;
                        let st = p.step(&mut m, dt);
                        engine.rpm = st.rpm;
                        plant = Some(st);
                        plant_cycle = m.cpu.cycles;
                    }
                }
            }
        }
        // CAN frames the ECU and the other nodes sent, in time order.
        let mut bus: Vec<(u64, Origin, Frame)> = m.chip.hcan.ch[0]
            .outbox
            .drain(..)
            .map(|(t, f)| (t, Origin::Ecu, f))
            .chain(m.chip.heard.drain(..).map(|(t, f)| (t, Origin::Node, f)))
            .collect();
        bus.sort_by_key(|e| e.0);
        for (t, from, frame) in bus {
            can.push_back(CanEvent {
                us: t / CYCLES_PER_US,
                from,
                frame,
            });
        }
        while can.len() > CAN_KEPT {
            can.pop_front();
        }
        if rt_wall.elapsed() >= Duration::from_secs(1) {
            let w = rt_wall.elapsed().as_secs_f64();
            rt_factor = (m.cpu.cycles - rt_cycle) as f64 / (CYCLES_PER_US as f64 * 1e6) / w;
            (rt_cycle, rt_wall) = (m.cpu.cycles, Instant::now());
        }
        if published.elapsed() >= Duration::from_millis(30) {
            let mut interrupts: Vec<(usize, u64)> = m
                .interrupts
                .iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .map(|(v, n)| (v, *n))
                .collect();
            interrupts.sort_by_key(|&(v, _)| v);
            let snap = Snapshot {
                cpu: m.cpu.clone(),
                running,
                stop,
                interrupts,
                can: can.clone(),
                mem_addr,
                mem: (0..MEM_WINDOW)
                    .map(|k| m.chip.peek(mem_addr.wrapping_add(k)))
                    .collect(),
                rt_factor: if running { rt_factor } else { 0.0 },
                adc: m.chip.adc.input,
                real_time,
                speed,
                crank_angle: m.chip.crank_angle(m.cpu.cycles),
                next_edge: m.chip.next_edge(),
                console: console.clone(),
                sci: std::array::from_fn(|k| {
                    let c = &m.chip.sci.ch[k];
                    (c.sent.len(), c.received.len())
                }),
                engine: engine.clone(),
                plant_mode,
                plant,
            };
            *shared.lock().expect("snapshot lock") = snap;
            published = Instant::now();
        }
    }
}

/// A virtual-time link that also records the frames for the CAN view.
struct LoggingLink<'a, 'b> {
    inner: VirtualLink<'a>,
    can: &'b mut VecDeque<CanEvent>,
}

impl bridge::link::CanLink for LoggingLink<'_, '_> {
    fn send(&mut self, frame: Frame) {
        let us = self.inner.now_us();
        self.can.push_back(CanEvent {
            us,
            from: Origin::Tool,
            frame,
        });
        self.inner.send(frame);
    }

    fn recv(&mut self, timeout_us: u64) -> Option<Frame> {
        let f = self.inner.recv(timeout_us)?;
        let us = self.inner.now_us();
        self.can.push_back(CanEvent {
            us,
            from: Origin::Ecu,
            frame: f,
        });
        Some(f)
    }

    fn now_us(&self) -> u64 {
        self.inner.now_us()
    }
}

/// A snapshot of a machine that has not run yet, with `engine` on its inputs.
pub fn first_snapshot(m: &Machine, engine: &Engine) -> Snapshot {
    Snapshot {
        cpu: m.cpu.clone(),
        running: false,
        stop: None,
        interrupts: Vec::new(),
        can: VecDeque::new(),
        mem_addr: sh7059::RAM_BASE,
        mem: vec![0; MEM_WINDOW as usize],
        rt_factor: 0.0,
        adc: m.chip.adc.input,
        real_time: true,
        speed: 1.0,
        crank_angle: m.chip.crank_angle(m.cpu.cycles),
        next_edge: m.chip.next_edge(),
        console: Vec::new(),
        sci: [(0, 0); 5],
        engine: engine.clone(),
        plant_mode: PlantMode::Off,
        plant: None,
    }
}
