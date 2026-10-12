//! The Android app's native side. As in the desktop window (ADR-0004), the emulated ECU
//! runs on a thread of its own that owns the machine ([`Live`]): the app sends it
//! [`Order`]s and reads what it publishes, a status and the engine's history, as one
//! flat array per frame ([`Live::frame`]). [`exports`] is the JNI bridge to
//! `com.varuna.sh705x.Native`.

mod exports;
mod maps;

use bridge::link::CYCLES_PER_US;
use session::maps::MapTrace;
use session::names::{Name, Profile};
use session::scope::{Scope, TRACES};
use session::{Engine, PlantMode, Session};
use sh7059::Stop;
use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Orders from the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Order {
    /// Run (true) or pause.
    Run(bool),
    /// ECU time per wall-clock time: 1 is real time, 0.001 a tooth at idle about every
    /// second.
    Speed(f64),
    /// The engine speed, rpm (0 stops it).
    Rpm(f64),
    /// A sensor by key, in physical units.
    Sensor(String, f64),
    /// Put the plants around the ECU, or take them away.
    Plants(PlantMode),
    /// All sensors and the speed at once: ignition on and still (false) or idling.
    Preset(bool),
    /// Paused, run to the next tooth position (6°).
    NextTooth,
    /// Paused, run to the next edge on the input pins.
    NextEdge,
    /// Trace the maps the firmware reads, or stop.
    TraceMaps(bool),
    /// Stop at the next lookup of this map (its descriptor), or no longer.
    BreakOnMap(Option<u32>),
    /// Name the variables: the sensors, and a profile's parameters if given.
    NameVariables(Option<Profile>),
    /// Map labels, by descriptor.
    LabelMaps(BTreeMap<u32, Name>),
    /// End the thread.
    Quit,
}

/// The maps being traced, as the thread publishes them.
type MapSlot = Arc<Mutex<Option<Arc<Mutex<MapTrace>>>>>;

/// What the thread published last.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// ECU time, µs.
    pub us: f64,
    /// Crankshaft angle over 720°, NaN with the engine stopped.
    pub crank: f64,
    /// Engine speed, rpm.
    pub rpm: f64,
    /// ECU time over wall time over the last second; 0 paused.
    pub rt_factor: f64,
    /// Running or paused.
    pub running: bool,
    /// ECU time per wall-clock time.
    pub speed: f64,
    /// The plants: 0 off, 1 held, 2 turning on the firmware's torque.
    pub plants: u8,
    /// Why it last stopped other than by being paused, if it did.
    pub stop: Option<String>,
}

/// Numbers before the columns in a [`Live::frame`].
pub const HEADER: usize = 9;
/// Numbers in a column: time, crank angle, then each trace's minimum and maximum.
pub const COLUMN: usize = 2 + 2 * TRACES.len();

/// The emulated ECU, running on its thread.
pub struct Live {
    orders: Sender<Order>,
    status: Arc<Mutex<Status>>,
    scope: Arc<Mutex<Scope>>,
    maps: MapSlot,
    thread: Option<JoinHandle<()>>,
}

/// A lock, also if a panic on the other side poisoned it: what it guards is plain data.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Live {
    /// Powers on an ECU on `flash` (and `eeprom`, if given), ignition on and the engine
    /// still, paused.
    pub fn open(flash: &[u8], eeprom: Option<&[u8]>) -> Result<Self, String> {
        let mut m = session::machine(flash, eeprom, board::DEFAULT_LAYOUT)?;
        let engine = Engine::ignition_on();
        engine.apply(&mut m);
        let ecu = Session::new(m, engine);
        let (orders, rx) = channel();
        let status = Arc::new(Mutex::new(Status {
            crank: f64::NAN,
            speed: 1.0,
            ..Status::default()
        }));
        let scope = Arc::new(Mutex::new(Scope::default()));
        let (s2, sc2) = (status.clone(), scope.clone());
        let maps: MapSlot = Arc::default();
        let m2 = maps.clone();
        let thread = std::thread::Builder::new()
            .name("ecu".into())
            .spawn(move || run(ecu, rx, s2, sc2, m2))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            orders,
            status,
            scope,
            maps,
            thread: Some(thread),
        })
    }

    /// Sends an order; it is lost only if the thread has ended.
    pub fn send(&self, o: Order) {
        let _ = self.orders.send(o);
    }

    /// What the thread published last.
    pub fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    /// The status and the last `window_us` of history, as one array:
    ///
    /// - [`HEADER`] numbers: ECU time (µs), crank angle, rpm, real-time factor, running
    ///   (1/0), speed, plants (0/1/2), the columns that follow, the pulses after them;
    /// - the window in up to `columns` columns of [`COLUMN`] numbers each: the time of
    ///   the column's last sample, its crank angle, then each trace's minimum and
    ///   maximum over the column, so a move faster than a column still shows;
    /// - the channel 8 pulses that end inside the window: output, start, end (µs).
    pub fn frame(&self, window_us: f64, columns: usize) -> Vec<f64> {
        let st = self.status();
        let sc = lock(&self.scope);
        let now = sc.samples.back().map_or(st.us, |s| s.us);
        let from = now - window_us;
        let mut cols: Vec<f64> = Vec::new();
        let mut n = 0;
        let mut current: Option<usize> = None;
        let first = sc.samples.partition_point(|s| s.us < from);
        for s in sc.samples.range(first..) {
            let col = (((s.us - from) / window_us * columns as f64) as usize)
                .min(columns.saturating_sub(1));
            if current != Some(col) {
                current = Some(col);
                n += 1;
                cols.extend([s.us, s.crank as f64]);
                cols.extend(std::iter::repeat_n(f64::NAN, 2 * TRACES.len()));
            }
            let at = cols.len() - COLUMN;
            cols[at] = s.us;
            cols[at + 1] = s.crank as f64;
            for (k, &v) in s.v.iter().enumerate() {
                let (lo, hi) = (at + 2 + 2 * k, at + 3 + 2 * k);
                cols[lo] = cols[lo].min(v as f64);
                cols[hi] = cols[hi].max(v as f64);
            }
        }
        let pulses: Vec<f64> = sc
            .pulses
            .iter()
            .filter(|p| p.2 >= from)
            .flat_map(|&(k, a, b)| [k as f64, a, b])
            .collect();
        let mut out = Vec::with_capacity(HEADER + cols.len() + pulses.len());
        out.extend([
            now,
            st.crank,
            st.rpm,
            st.rt_factor,
            if st.running { 1.0 } else { 0.0 },
            st.speed,
            st.plants as f64,
            n as f64,
            (pulses.len() / 3) as f64,
        ]);
        out.extend(cols);
        out.extend(pulses);
        out
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.send(Order::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The traces, one per line: name, a tab, unit.
pub fn traces() -> String {
    TRACES
        .iter()
        .map(|t| format!("{}\t{}", t.name, t.unit))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs the machine until told to quit, as the desktop window does (`gui` emu), always
/// paced to the wall clock.
fn run(
    mut ecu: Session,
    orders: Receiver<Order>,
    status: Arc<Mutex<Status>>,
    scope: Arc<Mutex<Scope>>,
    maps: MapSlot,
) {
    let mut running = false;
    let mut speed = 1.0f64;
    let mut stop: Option<Stop> = None;
    let (mut pace_cycle, mut pace_wall) = (ecu.m.cpu.cycles, Instant::now());
    let (mut rt_cycle, mut rt_wall, mut rt_factor) = (ecu.m.cpu.cycles, Instant::now(), 0.0);
    let mut published = Instant::now() - Duration::from_secs(1);
    loop {
        let first = if running {
            orders.try_recv().ok()
        } else {
            match orders.recv_timeout(Duration::from_millis(30)) {
                Ok(o) => Some(o),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };
        let mut next = first;
        while let Some(o) = next.take() {
            match o {
                Order::Run(r) => {
                    running = r;
                    if r {
                        stop = None;
                        ecu.resync();
                        (pace_cycle, pace_wall) = (ecu.m.cpu.cycles, Instant::now());
                    }
                }
                Order::Speed(s) => {
                    speed = s.clamp(1e-4, 1.0);
                    (pace_cycle, pace_wall) = (ecu.m.cpu.cycles, Instant::now());
                }
                Order::Rpm(r) => ecu.set_rpm(r),
                Order::Sensor(k, v) => {
                    let _ = ecu.set_sensor(&k, v);
                }
                Order::Plants(p) => ecu.set_plants(p),
                Order::Preset(idle) => ecu.preset(idle),
                Order::NextTooth => {
                    running = false;
                    stop = ecu.next_tooth();
                }
                Order::NextEdge => {
                    running = false;
                    stop = ecu.next_edge();
                }
                Order::TraceMaps(on) => {
                    ecu.trace_maps(on);
                    *lock(&maps) = ecu.maps.clone();
                }
                Order::BreakOnMap(d) => ecu.break_on_map(d),
                Order::NameVariables(p) => {
                    ecu.name_variables(p.as_ref());
                    (pace_cycle, pace_wall) = (ecu.m.cpu.cycles, Instant::now());
                }
                Order::LabelMaps(l) => ecu.label_maps(l),
                Order::Quit => return,
            }
            next = orders.try_recv().ok();
        }
        if running {
            let wall_us = pace_wall.elapsed().as_micros() as f64;
            let ahead =
                (ecu.m.cpu.cycles - pace_cycle) as f64 > wall_us * CYCLES_PER_US as f64 * speed;
            if ahead {
                std::thread::sleep(Duration::from_micros(500));
            } else {
                // Slowed down, in smaller slices, so the teeth come one by one.
                let s = ecu.run((40_000.0 * speed).max(200.0) as u64);
                if s != Stop::Count {
                    running = false;
                    stop = Some(s);
                }
            }
        }
        // Nobody reads the bus here: what the ECU and the other nodes sent is dropped.
        ecu.m.chip.hcan.ch[0].outbox.clear();
        ecu.m.chip.heard.clear();
        if rt_wall.elapsed() >= Duration::from_secs(1) {
            let w = rt_wall.elapsed().as_secs_f64();
            rt_factor = (ecu.m.cpu.cycles - rt_cycle) as f64 / (CYCLES_PER_US as f64 * 1e6) / w;
            (rt_cycle, rt_wall) = (ecu.m.cpu.cycles, Instant::now());
        }
        ecu.sample(&mut lock(&scope), !running);
        if published.elapsed() >= Duration::from_millis(30) {
            let m = &ecu.m;
            *lock(&status) = Status {
                us: m.cpu.cycles as f64 / CYCLES_PER_US as f64,
                crank: m.chip.crank_angle(m.cpu.cycles).unwrap_or(f64::NAN),
                rpm: ecu.engine.rpm,
                rt_factor: if running { rt_factor } else { 0.0 },
                running,
                speed,
                plants: match ecu.plant_mode {
                    PlantMode::Off => 0,
                    PlantMode::Held => 1,
                    PlantMode::Turning => 2,
                },
                stop: stop.filter(|s| *s != Stop::Count).map(|s| format!("{s:?}")),
            };
            published = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flash in `SH705X_FLASH`, or `None` to skip.
    fn flash() -> Option<Vec<u8>> {
        std::env::var("SH705X_FLASH")
            .ok()
            .map(|p| std::fs::read(p).expect("SH705X_FLASH"))
    }

    fn wait_for(live: &Live, what: impl Fn(&Status) -> bool) -> Status {
        let t = Instant::now();
        loop {
            let s = live.status();
            if what(&s) {
                return s;
            }
            assert!(t.elapsed() < Duration::from_secs(60), "timed out: {s:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_bad_image_is_refused() {
        assert!(Live::open(&[0; 16], None).is_err());
    }

    #[test]
    fn the_traces_are_named_with_their_units() {
        let t = traces();
        assert_eq!(t.lines().count(), TRACES.len());
        assert!(t.starts_with("engine speed\trpm"));
    }

    #[test]
    fn it_idles_on_its_torque_and_frames_the_history() {
        let Some(img) = flash() else { return };
        let live = Live::open(&img, None).unwrap();
        live.send(Order::Preset(true));
        live.send(Order::Plants(PlantMode::Turning));
        live.send(Order::Run(true));
        // Two seconds of ECU time: the crankshaft is turning on the firmware's torque.
        let s = wait_for(&live, |s| s.us > 2e6);
        assert!(s.running && s.plants == 2 && s.crank.is_finite(), "{s:?}");
        assert!(s.rpm > 300.0, "{s:?}");
        let f = live.frame(100_000.0, 50);
        let (cols, pulses) = (f[7] as usize, f[8] as usize);
        assert!(cols > 0 && cols <= 50, "{cols}");
        assert_eq!(f.len(), HEADER + cols * COLUMN + pulses * 3);
        // Each column's minimum is at most its maximum, and times go forward.
        let col = |c: usize| &f[HEADER + c * COLUMN..HEADER + (c + 1) * COLUMN];
        for c in 0..cols {
            let x = col(c);
            for k in 0..TRACES.len() {
                let (lo, hi) = (x[2 + 2 * k], x[3 + 2 * k]);
                assert!(lo.is_nan() || lo <= hi);
            }
            if c > 0 {
                assert!(x[0] > col(c - 1)[0]);
            }
        }
        // Paused, a tooth at a time.
        live.send(Order::Run(false));
        let a = wait_for(&live, |s| !s.running).crank;
        live.send(Order::NextTooth);
        let b = wait_for(&live, |s| s.crank != a).crank;
        let turned = (b - a).rem_euclid(720.0);
        assert!(turned > 0.0 && turned < 12.0, "{a} -> {b}");
    }

    #[test]
    fn the_maps_read_as_the_app_lays_them_out() {
        let Some(img) = flash() else { return };
        let live = Live::open(&img, None).unwrap();
        live.send(Order::Preset(true));
        live.send(Order::TraceMaps(true));
        live.send(Order::Speed(1.0));
        live.send(Order::Run(true));
        // The boost target (resultados fase 7) among the maps read.
        let t = Instant::now();
        while !live.map_list("09DADC").starts_with("09DADC") {
            assert!(t.elapsed() < Duration::from_secs(60), "timed out");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(live.maps_status().contains("routines 3D 7128, 2D 70B4"));
        let d = live.map_data(0x9DADC);
        let (nx, ny, trail) = (d[1] as usize, d[2] as usize, d[7] as usize);
        assert_eq!((d[0], nx, ny), (3.0, 20, 20));
        assert_eq!(
            d.len(),
            crate::maps::MAP_HEADER + nx + ny + nx * ny + 3 * trail
        );
        let text = live.map_text(0x9DADC);
        assert!(text.contains("from 03D852"), "{text}");
        assert!(text.contains("engine speed (routine 00D432)"), "{text}");
        assert!(live.map_data(0x1234).is_empty());
    }
}
