//! What the command line asks to be running and showing once the ECU is on: idling with
//! the plants around it, flat out up to a moment and slowed down from there, with the
//! engine view, the maps or the oscilloscope open.

use crate::app::App;
use crate::emu::{Cmd, PlantMode};

/// The options after the images. Without `--plants` or `--drive` the ECU waits, paused, as
/// ever.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Start {
    /// Idle (750 rpm, warm sensors) with these plants, and run.
    plants: Option<PlantMode>,
    /// A recorded drive.
    drive: Option<String>,
    /// ECU time run flat out before `speed` applies, ms.
    slow_at: Option<f64>,
    /// ECU time per wall-clock time.
    speed: Option<f64>,
    /// Windows to open.
    engine: bool,
    maps: bool,
    scope: bool,
    /// The engine view's charts, if not its usual ones, and its time window, ms.
    charts: Option<Vec<String>>,
    window: Option<f64>,
    /// The map shown (its descriptor).
    map: Option<u32>,
    /// A file of map labels.
    labels: Option<String>,
    /// A flash to compare the maps with.
    compare: Option<String>,
}

impl Start {
    /// Takes the option `name` with its value; `false` if it is not one of these.
    pub fn take(&mut self, name: &str, v: &str) -> Result<bool, String> {
        let number = || {
            v.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite())
                .ok_or_else(|| format!("{name}: not a number: {v}"))
        };
        match name {
            "--plants" => {
                self.plants = Some(match v {
                    "held" => PlantMode::Held,
                    "torque" => PlantMode::Turning,
                    _ => return Err(format!("--plants is held or torque, not {v}")),
                });
            }
            "--drive" => self.drive = Some(v.into()),
            "--slow-at" => self.slow_at = Some(number()?),
            "--speed" => self.speed = Some(number()?),
            "--show" => {
                for w in v.split(',') {
                    match w {
                        "engine" => self.engine = true,
                        "maps" => self.maps = true,
                        "scope" => self.scope = true,
                        _ => return Err(format!("--show takes engine, maps and scope, not {w}")),
                    }
                }
            }
            "--charts" => {
                let names: Vec<String> = v.split(',').map(str::to_string).collect();
                if let Some(n) = names
                    .iter()
                    .find(|n| !crate::view::CHARTS.contains(&n.as_str()))
                {
                    return Err(format!(
                        "--charts takes {}, not {n}",
                        crate::view::CHARTS.join(", ")
                    ));
                }
                (self.charts, self.engine) = (Some(names), true);
            }
            "--window" => (self.window, self.engine) = (Some(number()?), true),
            "--map" => {
                let d = u32::from_str_radix(v.trim_start_matches("0x"), 16)
                    .map_err(|_| format!("--map: not a hex address: {v}"))?;
                (self.map, self.maps) = (Some(d), true);
            }
            "--labels" => (self.labels, self.maps) = (Some(v.into()), true),
            "--compare" => (self.compare, self.maps) = (Some(v.into()), true),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Whether it opens windows: they take the whole screen.
    pub fn windows(&self) -> bool {
        self.engine || self.maps || self.scope
    }

    /// Sends the ECU its orders and opens the windows.
    pub fn apply(&self, app: &mut App) -> Result<(), String> {
        let read = |p: &str| std::fs::read(p).map_err(|e| format!("{p}: {e}"));
        let labels = self.labels.as_deref().map(read).transpose()?;
        let other = self.compare.as_deref().map(read).transpose()?;
        let drive = self.drive.as_deref().map(|p| {
            let text = String::from_utf8_lossy(&read(p)?).into_owned();
            session::drive::Drive::read(&text).map_err(|e| format!("{p}: {e}"))
        });
        let drive = drive.transpose()?;
        app.show(self.engine, self.maps, self.scope);
        if let Some(names) = &self.charts {
            app.view.only(names);
        }
        if let Some(ms) = self.window {
            app.view.nearest_window(ms);
        }
        if let Some(l) = labels {
            let l = session::names::read_labels(&String::from_utf8_lossy(&l));
            app.send(Cmd::LabelMaps(l));
        }
        if let Some(flash) = other {
            let path = std::path::Path::new(self.compare.as_deref().unwrap_or_default());
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            app.maps.compare_with(&name, flash);
        }
        if self.map.is_some() {
            app.maps.selected = self.map;
        }
        if let Some(mode) = self.plants {
            app.send(Cmd::Preset(true));
            app.send(Cmd::Plants(mode));
        }
        let driving = drive.is_some();
        if let Some(d) = drive {
            app.send(Cmd::Preset(true));
            app.send(Cmd::Drive(d));
        }
        match (self.slow_at, self.speed) {
            (Some(ms), v) => app.send(Cmd::SlowAt(ms, v.unwrap_or(1.0))),
            (None, Some(v)) => app.send(Cmd::Speed(v)),
            (None, None) => {}
        }
        if self.plants.is_some() || driving {
            app.send(Cmd::Run);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_takes_its_options_and_no_others() {
        let mut s = Start::default();
        for (k, v) in [
            ("--plants", "torque"),
            ("--slow-at", "1150"),
            ("--speed", "0.01"),
            ("--show", "engine,maps"),
            ("--map", "0x9B904"),
            ("--charts", "egr,air"),
            ("--window", "900"),
        ] {
            assert_eq!(s.take(k, v), Ok(true), "{k}");
        }
        assert_eq!(s.plants, Some(PlantMode::Turning));
        assert_eq!((s.slow_at, s.speed), (Some(1150.0), Some(0.01)));
        assert!(s.engine && s.maps && !s.scope);
        assert_eq!(s.map, Some(0x9B904));
        assert_eq!(s.take("--flash", "x"), Ok(false));
        for (k, v) in [
            ("--plants", "x"),
            ("--speed", "x"),
            ("--speed", "NaN"),
            ("--show", "x"),
            ("--map", "x"),
            ("--charts", "egr,x"),
        ] {
            assert!(s.take(k, v).is_err(), "{k}");
        }
    }
}
