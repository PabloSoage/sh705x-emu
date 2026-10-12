//! The emulation thread: it owns the machine, takes orders from the interface over a
//! channel, and publishes a snapshot about every 30 ms (ADR-0004).

use bridge::elm::Elm327;
use bridge::link::{CYCLES_PER_US, VirtualLink};
use engine::plants::State;
use session::Session;
use session::scope::Scope;
pub use session::{Engine, PlantMode};
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
    /// Trace the maps the firmware reads, or stop.
    TraceMaps(bool),
    /// Stop at the next lookup of this map (its descriptor), or no longer.
    BreakOnMap(Option<u32>),
    /// Name the variables the maps read and give: the sensors, and a profile's
    /// parameters if given.
    NameVariables(Option<session::names::Profile>),
    /// Map labels, by descriptor.
    LabelMaps(std::collections::BTreeMap<u32, session::names::Name>),
    /// End the thread.
    Quit,
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
    /// The history the engine view draws (the same one in every snapshot).
    pub scope: Arc<Mutex<Scope>>,
    /// The maps the firmware reads, while they are traced.
    pub maps: Option<Arc<Mutex<session::maps::MapTrace>>>,
    /// The map a map break waits for.
    pub map_break: Option<u32>,
}

/// Frames kept for the CAN view.
const CAN_KEPT: usize = 400;
/// Bytes in the memory window.
pub const MEM_WINDOW: u32 = 256;

/// Runs the machine until told to quit.
pub fn run(m: Machine, engine: Engine, cmds: Receiver<Cmd>, shared: Arc<Mutex<Snapshot>>) {
    let mut ecu = Session::new(m, engine);
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
    let (mut pace_cycle, mut pace_wall) = (ecu.m.cpu.cycles, Instant::now());
    // Real-time factor over the last second.
    let (mut rt_cycle, mut rt_wall, mut rt_factor) = (ecu.m.cpu.cycles, Instant::now(), 0.0);
    let mut published = Instant::now() - Duration::from_secs(1);
    let scope = shared.lock().expect("snapshot lock").scope.clone();
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
                    ecu.resync();
                    (pace_cycle, pace_wall) = (ecu.m.cpu.cycles, Instant::now());
                }
                Cmd::Pause => running = false,
                Cmd::Step(n) => {
                    running = false;
                    stop = Some(ecu.step(n));
                }
                Cmd::Breakpoints(b) => ecu.set_breakpoints(b),
                Cmd::TraceMaps(on) => ecu.trace_maps(on),
                Cmd::BreakOnMap(d) => ecu.break_on_map(d),
                Cmd::NameVariables(p) => {
                    ecu.name_variables(p.as_ref());
                    (pace_cycle, pace_wall) = (ecu.m.cpu.cycles, Instant::now());
                }
                Cmd::LabelMaps(l) => ecu.label_maps(l),
                Cmd::Adc(c, v) => ecu.m.chip.adc.input[c] = v,
                Cmd::Sensor(k, v) => {
                    let _ = ecu.set_sensor(k, v);
                }
                Cmd::Rpm(r) => ecu.set_rpm(r),
                Cmd::Plants(mode) => ecu.set_plants(mode),
                Cmd::Preset(idle) => ecu.preset(idle),
                Cmd::Memory(a) => mem_addr = a,
                Cmd::RealTime(r) => {
                    real_time = r;
                    (pace_cycle, pace_wall) = (ecu.m.cpu.cycles, Instant::now());
                }
                Cmd::Speed(s) => {
                    speed = s.clamp(1e-4, 1.0);
                    (pace_cycle, pace_wall) = (ecu.m.cpu.cycles, Instant::now());
                }
                Cmd::NextEdge => {
                    running = false;
                    stop = ecu.next_edge();
                }
                Cmd::NextTooth => {
                    running = false;
                    stop = ecu.next_tooth();
                }
                Cmd::Elm(line) => {
                    let us = ecu.m.cpu.cycles / CYCLES_PER_US;
                    let answer = {
                        let mut link = LoggingLink {
                            inner: VirtualLink::new(&mut ecu.m, 0),
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
                    (pace_cycle, pace_wall) = (ecu.m.cpu.cycles, Instant::now());
                    // The plants did not follow the exchange: pick up from here.
                    ecu.resync();
                }
                Cmd::Quit => return,
            }
            next = cmds.try_recv().ok();
        }
        if running {
            let ahead = real_time && {
                let wall_us = pace_wall.elapsed().as_micros() as u64;
                (ecu.m.cpu.cycles - pace_cycle) as f64
                    > wall_us as f64 * CYCLES_PER_US as f64 * speed
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
                let s = ecu.run(slice);
                if s != Stop::Count {
                    running = false;
                    stop = Some(s);
                }
            }
        }
        // CAN frames the ECU and the other nodes sent, in time order.
        let m = &mut ecu.m;
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
        ecu.sample(&mut scope.lock().expect("scope lock"), !running);
        if published.elapsed() >= Duration::from_millis(30) {
            let m = &ecu.m;
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
                engine: ecu.engine.clone(),
                plant_mode: ecu.plant_mode,
                plant: ecu.plant,
                scope: scope.clone(),
                maps: ecu.maps.clone(),
                map_break: ecu.map_break(),
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
        scope: Arc::default(),
        maps: None,
        map_break: None,
    }
}
