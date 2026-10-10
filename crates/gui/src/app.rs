//! The window: controls, registers, code around the PC, memory, CAN, interrupts, the
//! A/D inputs as sliders, and an ELM327 console.

use crate::emu::{Cmd, MEM_WINDOW, Snapshot};
use eframe::egui::{self, Color32, RichText};
use sh2e::disasm;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Which tab the bottom panel shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Memory,
    Can,
    Interrupts,
    Console,
}

/// The interface state.
pub struct App {
    shared: Arc<Mutex<Snapshot>>,
    cmds: Sender<Cmd>,
    flash: Arc<Vec<u8>>,
    breakpoints: Vec<u32>,
    tab: Tab,
    mem_text: String,
    bp_text: String,
    elm_text: String,
    /// Disassembly follows the PC, or stays at `code_at`.
    follow: bool,
    code_at: u32,
    code_text: String,
}

impl App {
    /// An interface on the emulation thread behind `shared` and `cmds`.
    pub fn new(shared: Arc<Mutex<Snapshot>>, cmds: Sender<Cmd>, flash: Arc<Vec<u8>>) -> Self {
        Self {
            shared,
            cmds,
            flash,
            breakpoints: Vec::new(),
            tab: Tab::Console,
            mem_text: format!("{:08X}", sh7059::RAM_BASE),
            bp_text: String::new(),
            elm_text: "0100".to_string(),
            follow: true,
            code_at: 0,
            code_text: String::new(),
        }
    }

    fn send(&self, c: Cmd) {
        let _ = self.cmds.send(c);
    }

    fn word(&self, addr: u32) -> Option<u16> {
        let a = addr as usize;
        (a + 1 < self.flash.len()).then(|| u16::from_be_bytes([self.flash[a], self.flash[a + 1]]))
    }

    fn toggle_breakpoint(&mut self, addr: u32) {
        if let Some(i) = self.breakpoints.iter().position(|&b| b == addr) {
            self.breakpoints.remove(i);
        } else {
            self.breakpoints.push(addr);
        }
        self.send(Cmd::Breakpoints(self.breakpoints.clone()));
    }
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(
        s.trim().trim_start_matches("0x").trim_start_matches("H'"),
        16,
    )
    .ok()
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(root);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.send(Cmd::Quit);
    }
}

impl App {
    /// Draws the whole window from the latest snapshot.
    pub fn draw(&mut self, root: &mut egui::Ui) {
        let s = self.shared.lock().expect("snapshot lock").clone();
        root.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));

        egui::Panel::top("controls").show(root, |ui| {
            ui.horizontal(|ui| {
                if s.running {
                    if ui.button("⏸ Pause").clicked() {
                        self.send(Cmd::Pause);
                    }
                } else if ui.button("▶ Run").clicked() {
                    self.send(Cmd::Run);
                }
                for n in [1u64, 100, 10_000] {
                    if ui
                        .add_enabled(!s.running, egui::Button::new(format!("Step {n}")))
                        .clicked()
                    {
                        self.send(Cmd::Step(n));
                    }
                }
                let mut rt = s.real_time;
                if ui.checkbox(&mut rt, "Real time").changed() {
                    self.send(Cmd::RealTime(rt));
                }
                ui.separator();
                let ms = s.cpu.cycles as f64 / 80_000.0;
                ui.label(format!(
                    "{} instructions · {:.3} s emulated · ×{:.2} real time",
                    s.cpu.instructions,
                    ms / 1000.0,
                    s.rt_factor
                ));
                if let Some(stop) = s.stop {
                    ui.label(RichText::new(format!("stopped: {stop:?}")).color(Color32::YELLOW));
                }
            });
        });

        egui::Panel::left("registers").show(root, |ui| {
            ui.heading("CPU");
            let c = &s.cpu;
            egui::Grid::new("regs").striped(true).show(ui, |ui| {
                for k in 0..16 {
                    ui.monospace(format!("R{k}"));
                    ui.monospace(format!("{:08X}", c.r[k]));
                    if k % 2 == 1 {
                        ui.end_row();
                    }
                }
                for (name, v) in [
                    ("PC", c.pc),
                    ("PR", c.pr),
                    ("SR", c.sr),
                    ("GBR", c.gbr),
                    ("VBR", c.vbr),
                    ("MACH", c.mach),
                    ("MACL", c.macl),
                    ("FPUL", c.fpul),
                    ("FPSCR", c.fpscr),
                ] {
                    ui.monospace(name);
                    ui.monospace(format!("{v:08X}"));
                    ui.end_row();
                }
            });
            ui.monospace(format!(
                "T={} S={} Q={} M={} I={}{}",
                c.sr & 1,
                (c.sr >> 1) & 1,
                (c.sr >> 8) & 1,
                (c.sr >> 9) & 1,
                c.interrupt_mask(),
                if c.sleeping { "  asleep" } else { "" }
            ));
            ui.separator();
            ui.label("FPU");
            egui::Grid::new("fregs").show(ui, |ui| {
                for k in 0..16 {
                    ui.monospace(format!("FR{k}"));
                    ui.monospace(format!("{:.6e}", f32::from_bits(c.fr[k])));
                    if k % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
            ui.separator();
            ui.label("SCI bytes sent / received");
            for (k, (tx, rx)) in s.sci.iter().enumerate() {
                if *tx > 0 {
                    ui.monospace(format!("SCI{k}: {tx} / {rx}"));
                }
            }
        });

        egui::Panel::right("sensors").show(root, |ui| {
            ui.heading("Engine");
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Ignition on").clicked() {
                        self.send(Cmd::Preset(false));
                    }
                    if ui.button("Idle").clicked() {
                        self.send(Cmd::Preset(true));
                    }
                });
                let mut rpm = s.engine.rpm;
                ui.horizontal(|ui| {
                    ui.monospace(format!("{:<13}", "rpm"));
                    if ui
                        .add(egui::Slider::new(&mut rpm, 0.0..=5000.0).step_by(10.0))
                        .changed()
                    {
                        self.send(Cmd::Rpm(rpm));
                    }
                });
                ui.label("Sensors as the ECU reports them:");
                for &(key, value) in &s.engine.sensors.values {
                    let mut v = value;
                    let (range, unit) = sensor_range(key);
                    ui.horizontal(|ui| {
                        ui.monospace(format!("{key:<13}")).on_hover_text(
                            engine::sensors::input(key).map_or("mass air flow", |i| i.name),
                        );
                        if ui
                            .add(egui::Slider::new(&mut v, range).suffix(format!(" {unit}")))
                            .changed()
                        {
                            self.send(Cmd::Sensor(key, v));
                        }
                    });
                }
                egui::CollapsingHeader::new("Raw A/D codes").show(ui, |ui| {
                    ui.label("10-bit codes; AN14 is the ignition input (on above 229).");
                    for k in 0..32 {
                        let mut v = s.adc[k];
                        ui.horizontal(|ui| {
                            ui.monospace(format!("AN{k:<2}"));
                            if ui.add(egui::Slider::new(&mut v, 0..=1023)).changed() {
                                self.send(Cmd::Adc(k, v));
                            }
                        });
                    }
                });
            });
        });

        egui::Panel::bottom("tabs")
            .resizable(true)
            .default_size(260.0)
            .min_size(200.0)
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Console, "ELM327 console");
                    ui.selectable_value(&mut self.tab, Tab::Can, "CAN");
                    ui.selectable_value(&mut self.tab, Tab::Memory, "Memory");
                    ui.selectable_value(&mut self.tab, Tab::Interrupts, "Interrupts");
                });
                ui.separator();
                match self.tab {
                    Tab::Console => {
                        ui.horizontal(|ui| {
                            let r = ui.text_edit_singleline(&mut self.elm_text);
                            let enter =
                                r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if ui.button("Send").clicked() || enter {
                                self.send(Cmd::Elm(self.elm_text.clone()));
                            }
                            ui.label("e.g. 0100, 010C, 03, 0902, ATSH7E0, 1A92");
                        });
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                for l in &s.console {
                                    ui.monospace(l);
                                }
                            });
                    }
                    Tab::Can => {
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                for e in &s.can {
                                    let f = &e.frame;
                                    let data: Vec<String> = f.data[..f.dlc.min(8) as usize]
                                        .iter()
                                        .map(|b| format!("{b:02X}"))
                                        .collect();
                                    let text = format!(
                                        "{:>12.3} ms  {}  {:03X}  [{}]  {}",
                                        e.us as f64 / 1000.0,
                                        if e.from_ecu { "ECU →" } else { "→ ECU" },
                                        f.id,
                                        f.dlc,
                                        data.join(" ")
                                    );
                                    let color = if e.from_ecu {
                                        Color32::LIGHT_GRAY
                                    } else {
                                        Color32::LIGHT_BLUE
                                    };
                                    ui.monospace(RichText::new(text).color(color));
                                }
                            });
                    }
                    Tab::Memory => {
                        ui.horizontal(|ui| {
                            ui.label("Address");
                            let r = ui.text_edit_singleline(&mut self.mem_text);
                            if r.lost_focus()
                                && let Some(a) = hex(&self.mem_text)
                            {
                                self.send(Cmd::Memory(a & !0xF));
                            }
                            ui.label("(peripheral registers show their last stored value)");
                        });
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for row in 0..MEM_WINDOW / 16 {
                                let a = s.mem_addr.wrapping_add(row * 16);
                                let b = &s.mem[(row * 16) as usize..(row * 16 + 16) as usize];
                                let hexs: Vec<String> =
                                    b.iter().map(|x| format!("{x:02X}")).collect();
                                let ascii: String = b
                                    .iter()
                                    .map(|&x| {
                                        if (0x20..0x7F).contains(&x) {
                                            x as char
                                        } else {
                                            '.'
                                        }
                                    })
                                    .collect();
                                ui.monospace(format!("{a:08X}  {}  {ascii}", hexs.join(" ")));
                            }
                        });
                    }
                    Tab::Interrupts => {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for (v, n) in &s.interrupts {
                                ui.monospace(format!("vector {v:3}: {n}"));
                            }
                        });
                    }
                }
            });

        egui::CentralPanel::default().show(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Code");
                ui.checkbox(&mut self.follow, "follow PC");
                ui.label("at");
                let r = ui.text_edit_singleline(&mut self.code_text);
                if r.lost_focus()
                    && let Some(a) = hex(&self.code_text)
                {
                    self.code_at = a & !1;
                    self.follow = false;
                }
                ui.label("breakpoints");
                let r = ui.text_edit_singleline(&mut self.bp_text);
                if r.lost_focus()
                    && let Some(a) = hex(&self.bp_text)
                {
                    self.toggle_breakpoint(a & !1);
                    self.bp_text.clear();
                }
            });
            ui.label(format!(
                "Click an address to toggle a breakpoint. Set: {}",
                self.breakpoints
                    .iter()
                    .map(|b| format!("{b:08X}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            ui.separator();
            let centre = if self.follow { s.cpu.pc } else { self.code_at };
            let start = centre.saturating_sub(2 * 16);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for k in 0..64u32 {
                    let a = start + 2 * k;
                    let Some(code) = self.word(a) else {
                        ui.monospace(format!("{a:08X}  (not in flash)"));
                        break;
                    };
                    let text = disasm::disassemble(code, a);
                    let note = match disasm::literal(code, a) {
                        Some((l, 4)) => {
                            let v = (self.word(l).unwrap_or(0) as u32) << 16
                                | self.word(l + 2).unwrap_or(0) as u32;
                            format!("  ; = H'{v:08X}")
                        }
                        Some((l, _)) => format!("  ; = H'{:04X}", self.word(l).unwrap_or(0)),
                        None => String::new(),
                    };
                    let bp = self.breakpoints.contains(&a);
                    let line = format!(
                        "{} {a:08X}  {code:04X}  {text}{note}",
                        if bp { "●" } else { " " }
                    );
                    let mut rt = RichText::new(line).monospace();
                    if a == s.cpu.pc {
                        rt = rt.background_color(Color32::from_rgb(60, 60, 20));
                    }
                    if bp {
                        rt = rt.color(Color32::LIGHT_RED);
                    }
                    if ui
                        .add(egui::Label::new(rt).sense(egui::Sense::click()))
                        .clicked()
                    {
                        self.toggle_breakpoint(a);
                    }
                }
            });
        });
    }
}

/// The slider range of a sensor: what its curve can report, and the unit.
fn sensor_range(key: &str) -> (std::ops::RangeInclusive<f64>, &'static str) {
    match engine::sensors::input(key) {
        Some(i) => {
            let lo = i.curve.iter().map(|p| p.1).fold(f64::MAX, f64::min);
            let hi = i.curve.iter().map(|p| p.1).fold(f64::MIN, f64::max);
            (lo..=hi, i.unit)
        }
        None => {
            let hi = engine::curves::MAF.iter().map(|p| p.1).fold(0.0, f64::max);
            (0.0..=hi, "g/s")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emu;
    use sh7059::{Machine, ROM_SIZE, Sh7059};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A flash whose reset vector points at an endless loop.
    fn flash() -> Vec<u8> {
        let mut f = vec![0u8; ROM_SIZE];
        f[0..4].copy_from_slice(&0x0000_1000u32.to_be_bytes());
        f[4..8].copy_from_slice(&0xFFFF_BFF0u32.to_be_bytes());
        // loop: ADD #1,R1; BRA loop; NOP
        for (i, c) in [0x7101u16, 0xAFFD, 0x0009].iter().enumerate() {
            f[0x1000 + 2 * i..0x1002 + 2 * i].copy_from_slice(&c.to_be_bytes());
        }
        f
    }

    fn wait_for(shared: &Arc<Mutex<Snapshot>>, what: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let t = Instant::now();
        loop {
            let s = shared.lock().unwrap().clone();
            if what(&s) {
                return s;
            }
            assert!(t.elapsed() < Duration::from_secs(20), "timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn the_emulation_thread_steps_runs_and_publishes() {
        let img = flash();
        let m = Machine::new(Sh7059::new(&img).unwrap(), true);
        let engine = emu::Engine::ignition_on();
        let shared = Arc::new(Mutex::new(emu::first_snapshot(&m, &engine)));
        let (tx, rx) = mpsc::channel();
        let s2 = shared.clone();
        let t = std::thread::spawn(move || emu::run(m, engine, rx, s2));
        tx.send(Cmd::Step(30)).unwrap();
        let s = wait_for(&shared, |s| s.cpu.instructions >= 30);
        assert_eq!(s.cpu.instructions, 30);
        assert!(!s.running);
        // A breakpoint on the loop's first instruction stops a run.
        tx.send(Cmd::Breakpoints(vec![0x1000])).unwrap();
        tx.send(Cmd::RealTime(false)).unwrap();
        tx.send(Cmd::Run).unwrap();
        let s = wait_for(&shared, |s| s.stop == Some(sh7059::Stop::Breakpoint));
        assert_eq!(s.stop, Some(sh7059::Stop::Breakpoint));
        assert_eq!(s.cpu.pc, 0x1000);
        // Inputs and the memory window.
        tx.send(Cmd::Adc(7, 512)).unwrap();
        tx.send(Cmd::Memory(0)).unwrap();
        let s = wait_for(&shared, |s| s.adc[7] == 512 && s.mem_addr == 0);
        assert_eq!(&s.mem[..4], &[0, 0, 0x10, 0], "the reset vector, peeked");
        // A sensor in physical units lands on its channel as a code.
        tx.send(Cmd::Sensor("coolant", 20.0)).unwrap();
        let code = engine::sensors::input("coolant").unwrap().code(20.0);
        let s = wait_for(&shared, |s| s.adc[7] == code);
        assert!(s.engine.sensors.values.contains(&("coolant", 20.0)));
        tx.send(Cmd::Preset(true)).unwrap();
        let s = wait_for(&shared, |s| s.engine.rpm == 750.0);
        assert_eq!(s.engine.sensors, engine::sensors::Sensors::idle());
        tx.send(Cmd::Quit).unwrap();
        t.join().unwrap();
    }

    /// One frame with no window: what would go to the GPU is dropped.
    fn frame(ctx: &egui::Context, app: &mut App) {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.draw(ui));
        out.textures_delta.clear();
        assert!(!out.shapes.is_empty(), "something was drawn");
    }

    #[test]
    fn every_tab_draws_without_a_window() {
        let img = flash();
        let m = Machine::new(Sh7059::new(&img).unwrap(), true);
        let mut snap = emu::first_snapshot(&m, &emu::Engine::ignition_on());
        snap.console = vec!["> 0100".into(), "    41 00 98 3B 20 13".into()];
        snap.can.push_back(emu::CanEvent {
            us: 12,
            from_ecu: true,
            frame: sh7059::hcan::Frame::standard(0x7E8, &[2, 0x41, 0]),
        });
        snap.interrupts = vec![(192, 3)];
        let shared = Arc::new(Mutex::new(snap));
        let (tx, rx) = mpsc::channel();
        let mut app = App::new(shared, tx, Arc::new(img));
        app.breakpoints.push(0x1002);
        let ctx = egui::Context::default();
        for tab in [Tab::Console, Tab::Can, Tab::Memory, Tab::Interrupts] {
            app.tab = tab;
            for _ in 0..2 {
                frame(&ctx, &mut app);
            }
        }
        // Code past the end of the flash shows as such rather than failing.
        app.follow = false;
        app.code_at = ROM_SIZE as u32 - 4;
        frame(&ctx, &mut app);
        assert!(rx.try_recv().is_err(), "drawing alone sends no order");
    }
}
