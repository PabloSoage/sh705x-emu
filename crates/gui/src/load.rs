//! The load screen: the flash image, the EEPROM image if there is one, and how its four
//! chips lie in the dump, chosen with the system's file dialog, typed in or dropped onto
//! the window. The command line's `--flash`, `--eeprom` and `--eeprom-layout` fill it in.

use eframe::egui::{self, RichText};
use std::path::{Path, PathBuf};

/// Files larger than this dropped onto the window are taken as the flash, smaller ones as
/// the EEPROM (1.5 MB against 8 KB).
const FLASH_ABOVE: u64 = 64 * 1024;

/// The images, read and checked for size, ready to power on.
pub struct Images {
    /// The flash.
    pub flash: Vec<u8>,
    /// The four EEPROMs, if given.
    pub eeprom: Option<Vec<u8>>,
    /// How the EEPROM dump's blocks map onto the chips.
    pub layout: board::Layout,
}

/// What the screen holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Load {
    /// Path of the flash image.
    pub flash: String,
    /// Path of the EEPROM image, empty for none.
    pub eeprom: String,
    /// The EEPROM layout, as `--eeprom-layout` takes it.
    pub layout: String,
    /// What went wrong the last time.
    pub error: Option<String>,
}

impl Default for Load {
    fn default() -> Self {
        Self {
            flash: String::new(),
            eeprom: String::new(),
            layout: layout_text(board::DEFAULT_LAYOUT),
            error: None,
        }
    }
}

/// `layout` as `--eeprom-layout` takes it: the dump's block for each chip, `le` if low
/// byte first.
pub fn layout_text(layout: board::Layout) -> String {
    let mut s: String = layout.blocks.iter().map(|b| b.to_string()).collect();
    if !layout.big_endian {
        s.push_str("le");
    }
    s
}

/// An EEPROM layout as `--eeprom-layout` takes it: four digits 0-3, the dump's block for
/// each chip, and `le` after them if the dump has the low byte first.
pub fn parse_layout(v: &str) -> Result<board::Layout, String> {
    let v = v.trim();
    let (digits, le) = v.strip_suffix("le").map_or((v, false), |d| (d, true));
    let b: Vec<usize> = digits
        .chars()
        .map(|c| c.to_digit(4).map(|d| d as usize))
        .collect::<Option<_>>()
        .ok_or_else(|| format!("bad EEPROM layout '{v}': four digits 0-3, then 'le' or not"))?;
    if b.len() != 4 {
        return Err(format!(
            "bad EEPROM layout '{v}': four digits 0-3, then 'le' or not"
        ));
    }
    Ok(board::Layout {
        blocks: [b[0], b[1], b[2], b[3]],
        big_endian: !le,
    })
}

impl Load {
    /// Reads and checks the images the screen names.
    pub fn read(&self) -> Result<Images, String> {
        let flash_path = self.flash.trim();
        if flash_path.is_empty() {
            return Err("choose a flash image".into());
        }
        let flash = std::fs::read(flash_path).map_err(|e| format!("{flash_path}: {e}"))?;
        // The chip checks the size, but says it better here.
        sh7059::Sh7059::new(&flash).map_err(|e| format!("{flash_path}: {e}"))?;
        let eeprom_path = self.eeprom.trim();
        let eeprom = if eeprom_path.is_empty() {
            None
        } else {
            let e = std::fs::read(eeprom_path).map_err(|e| format!("{eeprom_path}: {e}"))?;
            if e.len() != 4 * board::CHIP_BYTES {
                return Err(format!(
                    "{eeprom_path}: {} bytes; the board has four 93C86, {} bytes",
                    e.len(),
                    4 * board::CHIP_BYTES
                ));
            }
            Some(e)
        };
        Ok(Images {
            flash,
            eeprom,
            layout: parse_layout(&self.layout)?,
        })
    }

    /// Takes files dropped onto the window: the big one as the flash, a small one as the
    /// EEPROM.
    fn take_dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for p in dropped {
            let big = std::fs::metadata(&p).is_ok_and(|m| m.len() > FLASH_ABOVE);
            let s = p.display().to_string();
            if big {
                self.flash = s;
            } else {
                self.eeprom = s;
            }
            self.error = None;
        }
    }

    /// Draws the screen; the images once "Power on" has read them.
    pub fn draw(&mut self, root: &mut egui::Ui) -> Option<Images> {
        self.take_dropped(root.ctx());
        let mut images = None;
        egui::CentralPanel::default().show(root, |ui| {
            ui.add_space(24.0);
            ui.heading("SH705x emulator");
            ui.label(
                "Runs an SH705x ECU's own firmware on a simulated engine. Choose its flash \
                 image and, if you have it, its EEPROM image, or drop them onto the window.",
            );
            ui.add_space(12.0);
            egui::Grid::new("images")
                .num_columns(3)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Flash (1.5 MB)");
                    ui.add_sized(
                        [520.0, 20.0],
                        egui::TextEdit::singleline(&mut self.flash).hint_text("flash.bin"),
                    );
                    if ui.button("Choose…").clicked()
                        && let Some(p) = pick("Flash image", &self.flash)
                    {
                        self.flash = p;
                    }
                    ui.end_row();

                    ui.label("EEPROM (8 KB, optional)");
                    ui.add_sized(
                        [520.0, 20.0],
                        egui::TextEdit::singleline(&mut self.eeprom)
                            .hint_text("none: the ECU starts with blank EEPROMs"),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Choose…").clicked()
                            && let Some(p) = pick("EEPROM image", &self.eeprom)
                        {
                            self.eeprom = p;
                        }
                        if ui
                            .add_enabled(!self.eeprom.is_empty(), egui::Button::new("None"))
                            .clicked()
                        {
                            self.eeprom.clear();
                        }
                    });
                    ui.end_row();

                    ui.label("EEPROM layout");
                    ui.add(egui::TextEdit::singleline(&mut self.layout).desired_width(80.0))
                        .on_hover_text(
                            "The dump's block for each of the four chips, in the board's \
                             order, then 'le' if the dump has the low byte first. 0123 is \
                             a PCMflash dump as it is.",
                        );
                    if ui.button("Default").clicked() {
                        self.layout = layout_text(board::DEFAULT_LAYOUT);
                    }
                    ui.end_row();
                });
            ui.add_space(12.0);
            let ready = !self.flash.trim().is_empty();
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(RichText::new("▶ Power on").strong()),
                )
                .clicked()
            {
                match self.read() {
                    Ok(i) => {
                        self.error = None;
                        images = Some(i);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.add_space(12.0);
            ui.label(
                RichText::new(
                    "No firmware comes with the emulator: the images are read from your \
                     disk and never copied anywhere.",
                )
                .small()
                .weak(),
            );
        });
        images
    }
}

/// The system's open dialog, starting where `current` is.
fn pick(title: &str, current: &str) -> Option<String> {
    let mut d = rfd::FileDialog::new()
        .set_title(title)
        .add_filter("Dumps", &["bin", "ori", "eep", "rom"])
        .add_filter("All files", &["*"]);
    if let Some(dir) = Path::new(current.trim()).parent().filter(|d| d.is_dir()) {
        d = d.set_directory(dir);
    }
    d.pick_file().map(|p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_reads_as_on_the_command_line() {
        assert_eq!(parse_layout("0123").unwrap(), board::DEFAULT_LAYOUT);
        let l = parse_layout(" 3210le ").unwrap();
        assert_eq!(l.blocks, [3, 2, 1, 0]);
        assert!(!l.big_endian);
        assert_eq!(layout_text(l), "3210le");
        for bad in ["012", "01234", "0124", "abcd", ""] {
            assert!(parse_layout(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn it_names_what_is_wrong_with_the_images() {
        let mut l = Load::default();
        assert!(l.read().err().unwrap().contains("choose"));
        l.flash = "/no/such/flash.bin".into();
        assert!(l.read().err().unwrap().contains("/no/such/flash.bin"));
        let dir = std::env::temp_dir().join("sh705x-gui-load-test");
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small.bin");
        std::fs::write(&small, [0u8; 16]).unwrap();
        l.flash = small.display().to_string();
        assert!(l.read().is_err(), "a 16-byte flash");
    }
    fn harness(load: Load) -> egui_kittest::Harness<'static, Load> {
        egui_kittest::Harness::builder()
            .with_size(egui::vec2(1000.0, 250.0))
            .build_ui_state(
                |ui, l: &mut Load| {
                    l.draw(ui);
                },
                load,
            )
    }

    #[test]
    fn the_screen_draws_without_a_window() {
        let mut h = harness(Load {
            error: Some("flash.bin: no such file".into()),
            ..Load::default()
        });
        h.run_steps(2);
        assert_eq!(h.state().layout, "0123");
    }

    /// The load screen for the README (`cargo test -p gui --release -- --ignored screenshots`).
    #[test]
    #[ignore]
    fn screenshots_load() {
        let mut h = harness(Load {
            flash: "flash.bin".into(),
            ..Load::default()
        });
        h.run_steps(3);
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/img/gui-load.png");
        h.render().unwrap().save(&path).unwrap();
        eprintln!("{}", path.display());
    }
}
