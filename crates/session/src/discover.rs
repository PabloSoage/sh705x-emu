//! Naming RAM variables by watching the running ECU ([`Session::find_names`]): in the
//! state it is in, and again with everything moved one way and the other (every sensor to
//! a value of its own, the pedal pressed, and, with the speed held, the engine faster),
//! the variables that hold a sensor's value as the firmware reads it, or the value the ECU
//! answers for an identifier of a parameter profile, every time, are that sensor or that
//! parameter.
//! The engine is put back as it was.

use crate::names::{How, Name, Names, Parameter, Profile};
use crate::{PlantMode, Session};
use bridge::isotp::{Timing, request};
use bridge::link::VirtualLink;
use engine::sensors::{MAF_KEY, input};
use sh7059::Machine;
use std::collections::BTreeSet;

/// ECU time each state is held before it is read, instructions: about 1.5 s, so that
/// what the firmware filters (temperatures) gets there.
const SETTLE: u64 = 105_000_000;
/// Variables a match that moved between the states may name: copies of one value.
const COPIES: usize = 32;

/// Every finite float in RAM, by address.
fn ram_floats(m: &Machine) -> Vec<(u32, f32)> {
    let mut out = Vec::with_capacity(24 * 1024);
    let mut a = sh7059::RAM_BASE;
    loop {
        let bits = (0..4).fold(0u32, |v, k| v << 8 | m.chip.peek(a + k) as u32);
        let f = f32::from_bits(bits);
        if f.is_finite() {
            out.push((a, f));
        }
        match a.checked_add(4) {
            Some(n) if n <= 0xFFFF_FFFC => a = n,
            _ => break,
        }
    }
    out
}

/// The addresses in `ram` whose value is within `tol` of `v`.
fn near(ram: &[(u32, f32)], v: f64, tol: f64) -> BTreeSet<u32> {
    ram.iter()
        .filter(|&&(_, f)| (f as f64 - v).abs() <= tol)
        .map(|&(a, _)| a)
        .collect()
}

/// RAM as floats, by address.
type Ram = Vec<(u32, f32)>;

/// What one state gave: RAM, and each parameter's answer with RAM just after it.
struct State {
    ram: Ram,
    answers: Vec<Option<(f64, Ram)>>,
}

/// The value the firmware reads for sensor `key` set to `v`.
fn as_read(key: &str, v: f64) -> Option<f64> {
    if key == MAF_KEY {
        return Some(v);
    }
    input(key).map(|i| i.value(i.code(v)))
}

impl Session {
    /// Asks the ECU for `p`'s value on the emulated CAN bus: a mode 01 PID below 0x100,
    /// else a $22 DID, to the engine ECU (7E0).
    fn ask(&mut self, p: &Parameter) -> Option<f64> {
        let (payload, head): (Vec<u8>, Vec<u8>) = if p.pid < 0x100 {
            (vec![0x01, p.pid as u8], vec![0x41, p.pid as u8])
        } else {
            let (hi, lo) = ((p.pid >> 8) as u8, p.pid as u8);
            (vec![0x22, hi, lo], vec![0x62, hi, lo])
        };
        let timing = Timing {
            answer_us: 100_000,
            more_us: 0,
        };
        let answers = {
            let mut link = VirtualLink::new(&mut self.m, 0);
            request(&mut link, 0x7E0, &payload, &|id| id == 0x7E8, timing)
        };
        self.resync();
        let a = answers.first()?;
        a.payload
            .strip_prefix(head.as_slice())
            .and_then(|d| p.value(d))
    }

    /// RAM now, and each of `params` asked for.
    fn state(&mut self, params: &[Parameter]) -> State {
        let ram = ram_floats(&self.m);
        let answers = params
            .iter()
            .map(|p| self.ask(p).map(|v| (v, ram_floats(&self.m))))
            .collect();
        State { ram, answers }
    }

    /// Names the variables that are the sensors (as the firmware reads them) and, with
    /// `profile`, the parameters it lists, by reading the ECU as it is and with everything
    /// moved. Takes about five seconds of ECU time.
    pub fn find_names(&mut self, profile: Option<&Profile>) -> Names {
        let params: &[Parameter] = profile.map_or(&[], |p| &p.parameters);
        let (sensors, rpm) = (self.engine.sensors.clone(), self.engine.rpm);
        let held = self.plant_mode != PlantMode::Turning && rpm > 0.0;
        // Three states: as it is, everything moved one way (each sensor off any round
        // number, the pedal pressed, the engine faster), and the other way.
        let mut states = vec![(sensors.clone(), self.state(params))];
        for ((scale, add), pedal, faster) in [
            ((1.0731, 1.37), 25.0, 600.0),
            ((0.9317, -1.13), 12.0, 300.0),
        ] {
            for &(key, v) in &sensors.values {
                let _ = self.set_sensor(key, v * scale + add);
            }
            let _ = self.set_sensor("pedal", pedal);
            if held {
                self.set_rpm(rpm + faster);
            }
            self.run(SETTLE);
            let set = self.engine.sensors.clone();
            states.push((set, self.state(params)));
        }
        // As it was.
        self.engine.sensors = sensors.clone();
        self.engine.apply(&mut self.m);
        if held {
            self.set_rpm(rpm);
        }
        self.run(SETTLE / 4);

        let mut names = Names::default();
        for (i, &(key, _)) in sensors.values.iter().enumerate() {
            // What the firmware reads in each state.
            let read: Option<Vec<f64>> = states
                .iter()
                .map(|(set, _)| as_read(key, set.values[i].1))
                .collect();
            let Some(read) = read else { continue };
            let spread = read
                .iter()
                .flat_map(|x| read.iter().map(move |y| (x - y).abs()))
                .filter(|d| *d > 0.0)
                .fold(f64::INFINITY, f64::min);
            // The curves were measured as the ECU reports a sensor, rounded (a
            // temperature to a degree), and the variable holds it finer; but never so loose
            // that two states could be taken for each other. A sensor that hardly moves (a
            // switch) says nothing.
            let tol = |v: f64| (v.abs() * 1e-2).max(0.5).min(spread / 4.0);
            if !spread.is_finite() || spread < 1.0 {
                continue;
            }
            let mut found: Option<BTreeSet<u32>> = None;
            for ((_, st), &v) in states.iter().zip(&read) {
                let here = near(&st.ram, v, tol(v));
                found = Some(match found {
                    None => here,
                    Some(f) => f.intersection(&here).copied().collect(),
                });
            }
            let found = found.unwrap_or_default();
            if found.len() > COPIES {
                continue;
            }
            let (text, unit) = input(key).map_or(("MAF", "g/s"), |i| (i.name, i.unit));
            for addr in found {
                names.vars.entry(addr).or_insert_with(|| Name {
                    text: format!("{text} (sensor)"),
                    unit: unit.into(),
                    how: How::Sensor,
                });
            }
        }
        for (k, p) in params.iter().enumerate() {
            let answers: Vec<&(f64, Ram)> = states
                .iter()
                .filter_map(|(_, st)| st.answers[k].as_ref())
                .collect();
            if answers.len() < 2 || answers.iter().all(|(v, _)| *v == 0.0) {
                continue;
            }
            // An answer drops what is below its resolution.
            let t = |v: f64| p.scale.abs() * 1.01 + v.abs() * 1e-6;
            let (lo, hi) = answers
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), (v, _)| {
                    (l.min(*v), h.max(*v))
                });
            let sure = hi - lo > 2.0 * p.scale.abs();
            let mut found: Option<BTreeSet<u32>> = None;
            for (v, ram) in answers {
                let here = near(ram, *v, t(*v));
                found = Some(match found {
                    None => here,
                    Some(f) => f.intersection(&here).copied().collect(),
                });
            }
            let found = found.unwrap_or_default();
            if found.is_empty() || found.len() > if sure { COPIES } else { 1 } {
                continue;
            }
            for addr in found {
                names.vars.entry(addr).or_insert_with(|| Name {
                    text: p.name.clone(),
                    unit: p.unit.clone(),
                    how: How::Identifier { pid: p.pid, sure },
                });
            }
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use crate::names::{How, read_profiles};
    use crate::{Engine, Session};

    /// The Astra at warm idle, the engine held at 750 rpm, booted; `None` to skip.
    fn idling() -> Option<Session> {
        let f = std::fs::read(std::env::var("SH705X_FLASH").ok()?).ok()?;
        let mut m = crate::machine(&f, None, board::DEFAULT_LAYOUT).unwrap();
        let engine = Engine::idle();
        engine.apply(&mut m);
        let mut s = Session::new(m, engine);
        s.run(40_000_000);
        Some(s)
    }

    #[test]
    fn the_sensors_are_found_in_ram() {
        let Some(mut s) = idling() else { return };
        let names = s.find_names(None);
        let sensors: std::collections::BTreeSet<&str> =
            names.vars.values().map(|n| n.text.as_str()).collect();
        eprintln!("{} variables: {sensors:?}", names.vars.len());
        assert!(names.vars.values().all(|n| n.how == How::Sensor));
        assert!(sensors.len() >= 10, "{sensors:?}");
    }

    /// With an opendash parameter profile in `SH705X_PROFILE` (its "A17DTJ" block): the
    /// variables found for its identifiers, checked against two this project found
    /// another way (resultados fases 8 and 9).
    #[test]
    fn a_profile_names_the_variables_it_reads() {
        let Ok(path) = std::env::var("SH705X_PROFILE") else {
            return;
        };
        let Some(mut s) = idling() else { return };
        let profiles = read_profiles(&std::fs::read_to_string(path).unwrap());
        let p = profiles
            .iter()
            .find(|p| p.name.contains("A17DTJ"))
            .expect("an A17DTJ profile");
        let names = s.find_names(Some(p));
        let by_profile: std::collections::BTreeSet<&str> = names
            .vars
            .values()
            .filter(|n| matches!(n.how, How::Identifier { .. }))
            .map(|n| n.text.as_str())
            .collect();
        eprintln!(
            "{} of {} parameters: {by_profile:?}",
            by_profile.len(),
            p.parameters.len()
        );
        let named = |a: u32| names.vars.get(&a).map(|n| n.text.as_str());
        assert_eq!(named(0xFFFE_F924), Some("Desired Fuel Rail Pressure"));
        assert_eq!(named(0xFFFE_FC4C), Some("Torque Delivered Signal"));
    }
}
