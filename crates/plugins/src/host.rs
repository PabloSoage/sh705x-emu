//! Runs the plugins beside the machine: after each stretch the machine runs, each plugin
//! is given the frames the ECU sent and its tick, and the frames it sends come onto the
//! bus when they are due.

use crate::manifest::Manifest;
use sh7059::Machine;
use sh7059::hcan::Frame;
use std::collections::VecDeque;
use wasmi::{Caller, Config, Engine, Extern, Linker, Module, Store, TypedFunc};

/// Work one call of a plugin may do (wasmi fuel: about one unit an instruction).
pub const FUEL: u64 = 20_000_000;
/// Log lines kept per plugin.
pub const LOG_KEPT: usize = 200;
/// Frames a plugin may have waiting to go on the bus.
pub const QUEUE_KEPT: usize = 4096;

/// What a plugin's calls see and leave behind.
#[derive(Default)]
struct State {
    now_us: i64,
    /// The memory it reads, as of this call: (start, bytes).
    view: Vec<(u32, Vec<u8>)>,
    /// Frames to send: delay in microseconds, frame.
    out: Vec<(i64, Frame)>,
    /// RAM to write.
    writes: Vec<(u32, u8)>,
    log: Vec<String>,
}

/// A frame from the arguments of `send` or for `on_frame`.
fn frame(id: i32, flags: i32, len: i32, data: i64) -> Frame {
    let extended = flags & 1 != 0;
    Frame {
        id: id as u32 & if extended { 0x1FFF_FFFF } else { 0x7FF },
        extended,
        remote: flags & 2 != 0,
        dlc: len.clamp(0, 8) as u8,
        data: (data as u64).to_le_bytes(),
    }
}

/// The arguments of `on_frame` for a frame.
fn args(f: &Frame) -> (i32, i32, i32, i64) {
    (
        f.id as i32,
        f.extended as i32 | (f.remote as i32) << 1,
        f.dlc as i32,
        u64::from_le_bytes(f.data) as i64,
    )
}

/// One plugin loaded.
pub struct Plugin {
    /// What it says about itself.
    pub manifest: Manifest,
    /// Where it was loaded from (a file name).
    pub file: String,
    /// Why it was stopped, if it was: it is not called again.
    pub error: Option<String>,
    /// What it has logged, oldest first.
    pub log: VecDeque<String>,
    /// Frames from the ECU it was given.
    pub heard: u64,
    /// Frames it put on the bus.
    pub sent: u64,
    store: Store<State>,
    on_frame: Option<TypedFunc<(i32, i32, i32, i64), ()>>,
    tick: Option<TypedFunc<i64, ()>>,
    /// Cycle its next tick is due.
    next_tick: u64,
}

/// A plugin as a front end shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// What it says about itself.
    pub manifest: Manifest,
    /// Where it was loaded from.
    pub file: String,
    /// Why it was stopped, if it was.
    pub error: Option<String>,
    /// Its last log lines, oldest first.
    pub log: Vec<String>,
    /// Frames from the ECU it was given.
    pub heard: u64,
    /// Frames it put on the bus.
    pub sent: u64,
}

impl Plugin {
    /// Loads the module `wasm` (from `file`), links it and runs its `init`.
    fn load(engine: &Engine, wasm: &[u8], file: &str) -> Result<Self, String> {
        let manifest = Manifest::of(wasm)?;
        let module = Module::new(engine, wasm).map_err(|e| format!("not a valid module: {e}"))?;
        let mut store = Store::new(engine, State::default());
        let mut linker = <Linker<State>>::new(engine);
        let bad = |e: wasmi::errors::LinkerError| e.to_string();
        linker
            .func_wrap(
                "sh705x",
                "send",
                |mut c: Caller<'_, State>, id: i32, flags: i32, len: i32, data: i64, delay: i64| {
                    let s = c.data_mut();
                    if s.out.len() < QUEUE_KEPT {
                        s.out.push((delay.max(0), frame(id, flags, len, data)));
                    }
                },
            )
            .map_err(bad)?;
        linker
            .func_wrap("sh705x", "read", |c: Caller<'_, State>, addr: i32| -> i32 {
                let a = addr as u32;
                c.data()
                    .view
                    .iter()
                    .find_map(|(s, b)| {
                        a.checked_sub(*s)
                            .and_then(|o| b.get(o as usize))
                            .map(|&x| x as i32)
                    })
                    .unwrap_or(-1)
            })
            .map_err(bad)?;
        linker
            .func_wrap(
                "sh705x",
                "write",
                |mut c: Caller<'_, State>, addr: i32, value: i32| {
                    c.data_mut().writes.push((addr as u32, value as u8));
                },
            )
            .map_err(bad)?;
        linker
            .func_wrap(
                "sh705x",
                "log",
                |mut c: Caller<'_, State>, ptr: i32, len: i32| {
                    let mut buf = vec![0u8; len.clamp(0, 1024) as usize];
                    let read = c
                        .get_export("memory")
                        .and_then(Extern::into_memory)
                        .is_some_and(|m| m.read(&c, ptr as u32 as usize, &mut buf).is_ok());
                    let line = if read {
                        String::from_utf8_lossy(&buf).into_owned()
                    } else {
                        "(log: the text is outside its memory)".into()
                    };
                    c.data_mut().log.push(line);
                },
            )
            .map_err(bad)?;
        linker
            .func_wrap("sh705x", "now_us", |c: Caller<'_, State>| -> i64 {
                c.data().now_us
            })
            .map_err(bad)?;
        store.set_fuel(FUEL).map_err(|e| e.to_string())?;
        let instance = linker
            .instantiate(&mut store, &module)
            .and_then(|p| p.start(&mut store))
            .map_err(|e| format!("cannot be linked: {e}"))?;
        let on_frame = instance.get_typed_func(&store, "on_frame").ok();
        let tick = instance.get_typed_func(&store, "tick").ok();
        let mut p = Self {
            manifest,
            file: file.to_string(),
            error: None,
            log: VecDeque::new(),
            heard: 0,
            sent: 0,
            store,
            on_frame,
            tick,
            next_tick: 0,
        };
        if let Ok(init) = instance.get_typed_func::<(), ()>(&p.store, "init") {
            p.store.set_fuel(FUEL).map_err(|e| e.to_string())?;
            init.call(&mut p.store, ())
                .map_err(|e| format!("its init failed: {e}"))?;
            // What it logged shows at once, the machine running or not.
            let logged = std::mem::take(&mut p.store.data_mut().log);
            p.log.extend(logged);
        }
        Ok(p)
    }

    /// Its status.
    pub fn status(&self) -> Status {
        Status {
            manifest: self.manifest.clone(),
            file: self.file.clone(),
            error: self.error.clone(),
            log: self.log.iter().cloned().collect(),
            heard: self.heard,
            sent: self.sent,
        }
    }

    /// Runs `f` with a fresh budget; a trap or running out stops the plugin.
    fn call(&mut self, what: &str, f: impl FnOnce(&mut Store<State>) -> Result<(), wasmi::Error>) {
        if self.error.is_some() {
            return;
        }
        if let Err(e) = self.store.set_fuel(FUEL) {
            self.error = Some(e.to_string());
            return;
        }
        if let Err(e) = f(&mut self.store) {
            self.error = Some(format!("stopped in {what}: {e}"));
        }
    }
}

/// The plugins loaded, and the frames they have waiting.
#[derive(Default)]
pub struct Host {
    /// The plugins, in the order loaded.
    pub plugins: Vec<Plugin>,
    engine: Option<Engine>,
    /// Frames waiting to go on the bus: the cycle each is due, oldest first by due.
    queue: Vec<(u64, Frame)>,
}

impl Host {
    /// Loads the plugin `wasm` (from `file`); its index.
    pub fn load(&mut self, wasm: &[u8], file: &str) -> Result<usize, String> {
        let engine = self.engine.get_or_insert_with(|| {
            let mut c = Config::default();
            c.consume_fuel(true);
            Engine::new(&c)
        });
        let p = Plugin::load(engine, wasm, file)?;
        self.plugins.push(p);
        Ok(self.plugins.len() - 1)
    }

    /// Unloads plugin `index`.
    pub fn unload(&mut self, index: usize) {
        if index < self.plugins.len() {
            self.plugins.remove(index);
        }
    }

    /// The plugins as a front end shows them.
    pub fn status(&self) -> Vec<Status> {
        self.plugins.iter().map(Plugin::status).collect()
    }

    /// After the machine has run: gives each plugin the frames the ECU sent and its tick,
    /// writes the RAM they asked for, and puts their frames on the bus when due. A clock
    /// of `cycles_per_us`. Frames are delivered at the end of the stretch they fall due
    /// in, so as late as the stretch is long.
    pub fn step(&mut self, m: &mut Machine, cycles_per_us: u64) {
        if self.plugins.is_empty() && self.queue.is_empty() {
            m.chip.hcan.ch[0].tap = None;
            return;
        }
        let now = m.cpu.cycles;
        let now_us = (now / cycles_per_us) as i64;
        let sent: Vec<(u64, Frame)> = m.chip.hcan.ch[0]
            .tap
            .get_or_insert_with(VecDeque::new)
            .drain(..)
            .collect();
        for p in &mut self.plugins {
            if p.error.is_some() {
                continue;
            }
            let s = p.store.data_mut();
            s.now_us = now_us;
            s.view = p
                .manifest
                .reads
                .iter()
                .map(|&(a, b)| (a, (a..b).map(|x| m.chip.peek(x)).collect()))
                .collect();
            if let Some(f) = p.on_frame {
                for (_, fr) in &sent {
                    p.heard += 1;
                    p.call("on_frame", |st| f.call(st, args(fr)));
                }
            }
            if let Some(t) = p.tick
                && now >= p.next_tick
            {
                p.next_tick = now + p.manifest.tick_us * cycles_per_us;
                p.call("tick", |st| t.call(st, now_us));
            }
            let s = p.store.data_mut();
            for (delay, f) in s.out.drain(..) {
                if self.queue.len() < QUEUE_KEPT {
                    self.queue.push((now + delay as u64 * cycles_per_us, f));
                    p.sent += 1;
                }
            }
            for (a, v) in s.writes.drain(..) {
                if !m.chip.poke_ram(a, v) {
                    p.log
                        .push_back(format!("(write to {a:08X} refused: not RAM)"));
                }
            }
            for l in s.log.drain(..) {
                p.log.push_back(l);
            }
            while p.log.len() > LOG_KEPT {
                p.log.pop_front();
            }
        }
        self.queue.sort_by_key(|&(at, _)| at);
        let due = self.queue.partition_point(|&(at, _)| at <= now);
        for (at, f) in self.queue.drain(..due) {
            m.chip.can_deliver(0, &f);
            if m.chip.heard.len() < sh7059::HEARD_KEPT {
                m.chip.heard.push((at, f));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sh7059::Sh7059;

    /// A machine on an empty flash: nothing runs, the bus and RAM are there.
    fn machine() -> Machine {
        Machine::new(Sh7059::new(&vec![0; sh7059::ROM_SIZE]).unwrap(), true)
    }

    /// A plugin from its text, with `manifest` as its manifest.
    fn plugin(manifest: &str, body: &str) -> Vec<u8> {
        let m = manifest.replace('"', "\\\"");
        wat::parse_str(format!(
            r#"(module
                 (import "sh705x" "send" (func $send (param i32 i32 i32 i64 i64)))
                 (import "sh705x" "read" (func $read (param i32) (result i32)))
                 (import "sh705x" "write" (func $write (param i32 i32)))
                 (import "sh705x" "log" (func $log (param i32 i32)))
                 (memory (export "memory") 1)
                 (data (i32.const 0) "ticked")
                 {body}
                 (@custom "sh705x-plugin" "{m}"))"#
        ))
        .unwrap()
    }

    /// Echoes each frame the ECU sends one identifier up, 2 ms later; logs its ticks.
    #[test]
    fn a_plugin_hears_the_ecu_and_answers_on_the_bus() {
        let wasm = plugin(
            r#"{"name":"echo","version":"1","api":1,"tick_us":1000}"#,
            r#"(func (export "on_frame") (param $id i32) (param $f i32) (param $n i32) (param $d i64)
                 (call $send (i32.add (local.get $id) (i32.const 1)) (local.get $f) (local.get $n)
                   (local.get $d) (i64.const 2000)))
               (func (export "tick") (param i64) (call $log (i32.const 0) (i32.const 6)))"#,
        );
        let mut m = machine();
        let mut h = Host::default();
        h.load(&wasm, "echo.wasm").unwrap();
        assert_eq!(h.status()[0].manifest.name, "echo");
        h.step(&mut m, 80);
        assert_eq!(h.status()[0].log, ["ticked"]);
        let f = Frame::standard(0x7E8, &[2, 0x41, 0x0C]);
        m.chip.hcan.ch[0].tap.as_mut().unwrap().push_back((0, f));
        h.step(&mut m, 80);
        assert!(m.chip.heard.is_empty(), "not due yet");
        m.cpu.cycles += 2000 * 80;
        h.step(&mut m, 80);
        let (_, back) = m.chip.heard[0];
        assert_eq!(
            (back.id, back.dlc, &back.data[..3]),
            (0x7E9, 3, &[2, 0x41, 0x0C][..])
        );
        let s = &h.status()[0];
        assert_eq!((s.heard, s.sent, s.error.clone()), (1, 1, None));
    }

    /// Reads the RAM its manifest names, writes RAM after the call, and nothing else.
    #[test]
    fn a_plugin_reads_what_it_declares_and_writes_ram() {
        let wasm = plugin(
            r#"{"name":"rw","version":"1","api":1,"reads":[["FFFE8000","FFFE8004"]]}"#,
            r#"(func (export "tick") (param i64)
                 (call $write (i32.const 0xFFFE8010) (i32.add (call $read (i32.const 0xFFFE8001)) (i32.const 1)))
                 (call $write (i32.const 0xFFFE8011) (call $read (i32.const 0xFFFE8010)))
                 (call $write (i32.const 0x1000) (i32.const 7)))"#,
        );
        let mut m = machine();
        m.chip.poke_ram(0xFFFE_8001, 41);
        let mut h = Host::default();
        h.load(&wasm, "rw.wasm").unwrap();
        h.step(&mut m, 80);
        assert_eq!(m.chip.peek(0xFFFE_8010), 42);
        // Outside what it reads: -1, written as 0xFF.
        assert_eq!(m.chip.peek(0xFFFE_8011), 0xFF);
        assert_eq!(m.chip.peek(0x1000), 0);
        assert!(h.status()[0].log[0].contains("refused"));
    }

    /// What its init logs shows as soon as it is loaded, before the machine runs.
    #[test]
    fn what_init_logs_shows_at_once() {
        let wasm = plugin(
            r#"{"name":"hello","version":"1","api":1}"#,
            r#"(func (export "init") (call $log (i32.const 0) (i32.const 6)))"#,
        );
        let mut h = Host::default();
        h.load(&wasm, "hello.wasm").unwrap();
        assert_eq!(h.status()[0].log, ["ticked"]);
    }

    /// A plugin that never returns is stopped, and the others go on.
    #[test]
    fn a_runaway_plugin_is_stopped() {
        let spin = plugin(
            r#"{"name":"spin","version":"1","api":1}"#,
            r#"(func (export "tick") (param i64) (loop (br 0)))"#,
        );
        let mut m = machine();
        let mut h = Host::default();
        h.load(&spin, "spin.wasm").unwrap();
        h.step(&mut m, 80);
        let e = h.status()[0].error.clone().unwrap();
        assert!(e.contains("tick"), "{e}");
        assert!(h.load(b"\0asm\x01\0\0\0", "bare.wasm").is_err());
    }
}
