//! `sh705x-gui`: watch the emulated ECU run. Registers, the code around the PC with
//! breakpoints, memory, CAN traffic, interrupts, the engine (speed and sensors in
//! physical units, and the raw A/D codes) as sliders, and an ELM327 console on the
//! emulated CAN bus.
//!
//! It opens on a load screen to choose the images; given on the command line, it powers
//! on with them straight away:
//!
//! ```text
//! sh705x-gui [--flash <flash.bin>] [--eeprom <eeprom.bin>] [--eeprom-layout 0123[le]]
//! ```

// A window, not a console program, in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod emu;
mod faultview;
mod load;
mod mapview;
mod view;

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

fn main() -> Result<(), String> {
    let mut load = load::Load::default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let v = it.next().ok_or_else(|| format!("{a} needs a value"))?;
        match a.as_str() {
            "--flash" => load.flash = v.clone(),
            "--eeprom" => load.eeprom = v.clone(),
            "--eeprom-layout" => {
                load::parse_layout(v)?;
                load.layout = v.clone();
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    let shell = Shell::new(load);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_title("SH705x emulator"),
        ..Default::default()
    };
    eframe::run_native(
        "sh705x-gui",
        options,
        Box::new(move |_| Ok(Box::new(shell))),
    )
    .map_err(|e| e.to_string())
}

/// What the window shows: the load screen, or an ECU running.
enum Screen {
    Load(load::Load),
    Running(Box<Running>),
}

/// An ECU running on its thread, the window on it, and the load screen to go back to.
struct Running {
    app: app::App,
    cmds: Sender<emu::Cmd>,
    thread: Option<JoinHandle<()>>,
    load: load::Load,
}

impl Running {
    /// Powers an ECU on with `images`, ignition on, the engine still, paused.
    fn start(images: load::Images, load: load::Load) -> Result<Self, String> {
        let mut m = session::machine(&images.flash, images.eeprom.as_deref(), images.layout)?;
        let engine = emu::Engine::ignition_on();
        engine.apply(&mut m);
        let shared = Arc::new(Mutex::new(emu::first_snapshot(&m, &engine)));
        let (cmds, rx) = mpsc::channel();
        let s2 = shared.clone();
        let thread = std::thread::spawn(move || emu::run(m, engine, rx, s2));
        Ok(Self {
            app: app::App::new(shared, cmds.clone(), Arc::new(images.flash)),
            cmds,
            thread: Some(thread),
            load,
        })
    }

    /// Ends the emulation thread.
    fn stop(&mut self) {
        let _ = self.cmds.send(emu::Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The window around the screens.
struct Shell {
    screen: Screen,
}

impl Shell {
    /// Powered on with `load`'s images if it names a flash and they read, else on the load
    /// screen (saying what was wrong, if anything).
    fn new(mut load: load::Load) -> Self {
        if load.flash.is_empty() {
            return Self {
                screen: Screen::Load(load),
            };
        }
        let screen = match load.read().and_then(|i| Running::start(i, load.clone())) {
            Ok(r) => Screen::Running(Box::new(r)),
            Err(e) => {
                load.error = Some(e);
                Screen::Load(load)
            }
        };
        Self { screen }
    }
}

impl eframe::App for Shell {
    fn ui(&mut self, root: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        let next = match &mut self.screen {
            Screen::Load(l) => l
                .draw(root)
                .map(|images| match Running::start(images, l.clone()) {
                    Ok(r) => Screen::Running(Box::new(r)),
                    Err(e) => {
                        let mut l = l.clone();
                        l.error = Some(e);
                        Screen::Load(l)
                    }
                }),
            Screen::Running(r) => {
                r.app.draw(root);
                r.app.back.then(|| {
                    r.stop();
                    Screen::Load(r.load.clone())
                })
            }
        };
        if let Some(s) = next {
            self.screen = s;
            root.ctx().request_repaint();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Screen::Running(r) = &mut self.screen {
            r.stop();
        }
    }
}
