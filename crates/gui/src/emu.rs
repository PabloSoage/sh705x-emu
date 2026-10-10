//! The emulation thread: it owns the machine, takes orders from the interface over a
//! channel, and publishes a snapshot about every 30 ms (ADR-0004).

use bridge::elm::Elm327;
use bridge::link::{CYCLES_PER_US, VirtualLink};
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
    /// Follow the wall clock (true) or run flat out.
    RealTime(bool),
    /// Send an ELM327 command line over the emulated CAN bus, in virtual time.
    Elm(String),
    /// End the thread.
    Quit,
}

/// One CAN frame seen on HCAN0: when, which way, what.
#[derive(Clone, Copy, Debug)]
pub struct CanEvent {
    /// Emulated time, µs.
    pub us: u64,
    /// Sent by the ECU (true) or by the tool (false).
    pub from_ecu: bool,
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
    /// Following the wall clock.
    pub real_time: bool,
    /// The ELM327 console: commands and answers.
    pub console: Vec<String>,
    /// Bytes sent and received on each SCI channel.
    pub sci: [(usize, usize); 5],
    /// The engine as set: speed and sensors.
    pub engine: Engine,
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

/// Frames kept for the CAN view.
const CAN_KEPT: usize = 400;
/// Bytes in the memory window.
pub const MEM_WINDOW: u32 = 256;

/// Runs the machine until told to quit.
pub fn run(mut m: Machine, mut engine: Engine, cmds: Receiver<Cmd>, shared: Arc<Mutex<Snapshot>>) {
    let mut running = false;
    let mut real_time = true;
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
                }
                Cmd::Quit => return,
            }
            next = cmds.try_recv().ok();
        }
        if running {
            let ahead = real_time && {
                let wall_us = pace_wall.elapsed().as_micros() as u64;
                m.cpu.cycles - pace_cycle > wall_us * CYCLES_PER_US
            };
            if ahead {
                std::thread::sleep(Duration::from_micros(500));
            } else {
                let s = m.run_until(40_000);
                if s != Stop::Count {
                    running = false;
                    stop = Some(s);
                }
            }
        }
        // CAN frames the ECU sent.
        for (t, f) in m.chip.hcan.ch[0].outbox.drain(..) {
            can.push_back(CanEvent {
                us: t / CYCLES_PER_US,
                from_ecu: true,
                frame: f,
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
                console: console.clone(),
                sci: std::array::from_fn(|k| {
                    let c = &m.chip.sci.ch[k];
                    (c.sent.len(), c.received.len())
                }),
                engine: engine.clone(),
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
            from_ecu: false,
            frame,
        });
        self.inner.send(frame);
    }

    fn recv(&mut self, timeout_us: u64) -> Option<Frame> {
        let f = self.inner.recv(timeout_us)?;
        let us = self.inner.now_us();
        self.can.push_back(CanEvent {
            us,
            from_ecu: true,
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
        console: Vec::new(),
        sci: [(0, 0); 5],
        engine: engine.clone(),
    }
}
