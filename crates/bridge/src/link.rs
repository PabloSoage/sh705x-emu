//! The diagnostic tool's side of the CAN bus to the emulated ECU.
//!
//! [`VirtualLink`] runs the machine itself while it waits, in virtual time, so a session
//! is deterministic and goes as fast as the emulator. [`RealTimeEcu`] runs the machine in
//! its own thread, paced to the wall clock, for tools on the other end of a socket.

use sh7059::Machine;
use sh7059::hcan::Frame;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// CPU cycles per microsecond: φ = 80 MHz, as the firmware's own timing implies.
pub const CYCLES_PER_US: u64 = 80;

/// The tool's bus speed, bits per second: the 500 kbit/s the ECU is set to.
pub const BUS_BPS: u64 = 500_000;

/// CPU cycles a frame from the tool takes on the bus before the ECU has it.
pub fn wire_cycles(f: &Frame) -> u64 {
    f.bits() * CYCLES_PER_US * 1_000_000 / BUS_BPS
}

/// A CAN bus as a diagnostic tool sees it.
pub trait CanLink {
    /// Puts a frame on the bus.
    fn send(&mut self, frame: Frame);
    /// The next frame from the ECU, waiting at most `timeout_us` of bus time.
    fn recv(&mut self, timeout_us: u64) -> Option<Frame>;
    /// Bus time now, in microseconds from an arbitrary start.
    fn now_us(&self) -> u64;
}

/// Runs the machine while waiting for frames: virtual time, deterministic.
pub struct VirtualLink<'a> {
    /// The ECU.
    pub machine: &'a mut Machine,
    /// HCAN channel the tool is on.
    pub channel: usize,
    /// Frames from the ECU not read yet.
    pending: VecDeque<Frame>,
    /// Frames from the tool on the wire, with the cycle each is complete.
    inbound: VecDeque<(u64, Frame)>,
}

impl<'a> VirtualLink<'a> {
    /// A link to `machine` on HCAN channel `channel`. Frames the ECU sent before now are
    /// dropped.
    pub fn new(machine: &'a mut Machine, channel: usize) -> Self {
        machine.chip.hcan.ch[channel].outbox.clear();
        Self {
            machine,
            channel,
            pending: VecDeque::new(),
            inbound: VecDeque::new(),
        }
    }
}

impl CanLink for VirtualLink<'_> {
    fn now_us(&self) -> u64 {
        self.machine.cpu.cycles / CYCLES_PER_US
    }

    /// The frame reaches the ECU once its bits are through, after any frame the tool
    /// sent before it.
    fn send(&mut self, frame: Frame) {
        let start = self
            .inbound
            .back()
            .map_or(self.machine.cpu.cycles, |&(t, _)| t)
            .max(self.machine.cpu.cycles);
        self.inbound.push_back((start + wire_cycles(&frame), frame));
    }

    fn recv(&mut self, timeout_us: u64) -> Option<Frame> {
        let deadline = self.machine.cpu.cycles + timeout_us * CYCLES_PER_US;
        loop {
            while let Some(&(t, f)) = self.inbound.front() {
                if t > self.machine.cpu.cycles {
                    break;
                }
                self.inbound.pop_front();
                self.machine.chip.can_deliver(self.channel, &f);
            }
            if let Some(f) = self.pending.pop_front() {
                return Some(f);
            }
            if self.machine.cpu.cycles >= deadline {
                return None;
            }
            // About 25 µs of emulation at a time: short next to any diagnostic timeout.
            self.machine.run(2_000);
            let out = &mut self.machine.chip.hcan.ch[self.channel].outbox;
            self.pending.extend(out.drain(..).map(|(_, f)| f));
        }
    }
}

/// How the real-time thread is doing.
#[derive(Debug, Default)]
pub struct Stats {
    /// Emulated CPU cycles.
    pub cycles: AtomicU64,
    /// Wall-clock microseconds since start.
    pub wall_us: AtomicU64,
    /// Microseconds the emulation was behind the wall clock at its worst.
    pub worst_lag_us: AtomicU64,
}

/// The machine running in its own thread, paced to the wall clock (or as fast as it
/// goes), with the bus to it over channels.
pub struct RealTimeEcu {
    to_ecu: Sender<Frame>,
    from_ecu: Receiver<Frame>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Machine>>,
    /// Progress, for showing the real-time factor.
    pub stats: Arc<Stats>,
    /// When it started, for [`CanLink::now_us`].
    start: Instant,
}

/// Frames from the ECU kept for a tool that has not read them; later ones are dropped.
const BACKLOG: usize = 4096;

impl RealTimeEcu {
    /// Starts `machine` on HCAN channel `channel`. With `paced`, emulated time follows
    /// the wall clock; without, it runs flat out.
    pub fn spawn(mut machine: Machine, channel: usize, paced: bool) -> Self {
        let (to_ecu, ecu_rx) = mpsc::channel::<Frame>();
        let (ecu_tx, from_ecu) = mpsc::sync_channel::<Frame>(BACKLOG);
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Stats::default());
        let (stop2, stats2) = (stop.clone(), stats.clone());
        let thread = std::thread::spawn(move || {
            let start = Instant::now();
            let c0 = machine.cpu.cycles;
            machine.chip.hcan.ch[channel].outbox.clear();
            // Frames from the tool on the wire: each reaches the ECU once its bits are
            // through, after the one before it.
            let mut inbound: VecDeque<(u64, Frame)> = VecDeque::new();
            while !stop2.load(Ordering::Relaxed) {
                while let Ok(f) = ecu_rx.try_recv() {
                    let from = inbound
                        .back()
                        .map_or(machine.cpu.cycles, |&(t, _)| t)
                        .max(machine.cpu.cycles);
                    inbound.push_back((from + wire_cycles(&f), f));
                }
                while let Some(&(t, f)) = inbound.front() {
                    if t > machine.cpu.cycles {
                        break;
                    }
                    inbound.pop_front();
                    machine.chip.can_deliver(channel, &f);
                }
                let wall = start.elapsed().as_micros() as u64;
                let target = c0 + wall * CYCLES_PER_US;
                let done = machine.cpu.cycles;
                if paced && done >= target {
                    std::thread::sleep(Duration::from_micros(200));
                    continue;
                }
                if done < target {
                    let lag = (target - done) / CYCLES_PER_US;
                    stats2.worst_lag_us.fetch_max(lag, Ordering::Relaxed);
                }
                // About 100 µs of emulation, then look at the bus again.
                machine.run(8_000);
                for (_, f) in machine.chip.hcan.ch[channel].outbox.drain(..) {
                    match ecu_tx.try_send(f) {
                        Ok(()) | Err(TrySendError::Full(_)) => {}
                        Err(TrySendError::Disconnected(_)) => break,
                    }
                }
                stats2
                    .cycles
                    .store(machine.cpu.cycles - c0, Ordering::Relaxed);
                stats2.wall_us.store(wall, Ordering::Relaxed);
            }
            machine
        });
        Self {
            to_ecu,
            from_ecu,
            stop,
            thread: Some(thread),
            stats,
            start: Instant::now(),
        }
    }

    /// Emulated time over wall time so far: 1.0 is real time.
    pub fn real_time_factor(&self) -> f64 {
        let c = self.stats.cycles.load(Ordering::Relaxed) as f64;
        let w = self.stats.wall_us.load(Ordering::Relaxed) as f64;
        if w == 0.0 {
            0.0
        } else {
            c / CYCLES_PER_US as f64 / w
        }
    }

    /// Stops the thread and gives the machine back.
    pub fn stop(mut self) -> Machine {
        self.stop.store(true, Ordering::Relaxed);
        self.thread
            .take()
            .expect("running")
            .join()
            .expect("emulation thread")
    }

    /// Drops what the ECU sent before now, so a request only sees its own answers.
    pub fn flush(&mut self) {
        while self.from_ecu.try_recv().is_ok() {}
    }
}

impl CanLink for RealTimeEcu {
    fn now_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    fn send(&mut self, frame: Frame) {
        let _ = self.to_ecu.send(frame);
    }

    fn recv(&mut self, timeout_us: u64) -> Option<Frame> {
        self.from_ecu
            .recv_timeout(Duration::from_micros(timeout_us))
            .ok()
    }
}

impl Drop for RealTimeEcu {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A shared real-time ECU, for several connections one after another.
pub type SharedEcu = Arc<Mutex<RealTimeEcu>>;
