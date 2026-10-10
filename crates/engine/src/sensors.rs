//! Engine sensors as the ECU's analog inputs: physical values in, 10-bit A/D codes out.
//!
//! Each input has the transfer curve the firmware itself applies ([`crate::curves`]):
//! the value it reports for 33 codes. A value is turned into a code by walking the
//! curve backwards, so whatever is set here is what a diagnostic tool reads back, to
//! the resolution of the converter.
//!
//! [`Sensors::ignition_on`] is a warm engine that is not turning; [`Sensors::idle`] the
//! same engine idling. Both take their values from the Astra's own recordings at warm
//! idle (scanmatik-bench routes 67, 69 and 72: medians with 700-1000 rpm, standing
//! still and coolant at 70 degC or more) [MED], except where noted.

use crate::curves::{INPUTS, MAF};

/// The mass air flow meter's key: a frequency input (TI0C), not an A/D one.
pub const MAF_KEY: &str = "maf";

/// One analog input and its curve.
#[derive(Debug)]
pub struct Input {
    /// A/D channel (AN0-AN31).
    pub channel: usize,
    /// Short name, for command lines.
    pub key: &'static str,
    /// What it is.
    pub name: &'static str,
    /// Unit of the values.
    pub unit: &'static str,
    /// (code, value the firmware reports), codes rising.
    pub curve: &'static [(u16, f64)],
}

impl Input {
    /// The value the firmware reports for `code`, interpolated.
    pub fn value(&self, code: u16) -> f64 {
        let c = self.curve;
        let k = c.partition_point(|&(x, _)| x <= code).clamp(1, c.len() - 1);
        let ((x0, y0), (x1, y1)) = (c[k - 1], c[k]);
        y0 + (y1 - y0) * (code as f64 - x0 as f64) / (x1 as f64 - x0 as f64)
    }

    /// A code for which the firmware reports `value`, or the nearest it can. Where the
    /// curve is flat (a clamp), the code nearest the slope is taken.
    pub fn code(&self, value: f64) -> u16 {
        let c: Vec<(f64, f64)> = self.curve.iter().map(|&(x, y)| (x as f64, y)).collect();
        inverse(&c, value).round() as u16
    }
}

/// The x for which a sampled curve gives `value`, interpolated; out of range or where
/// the curve is flat (a clamp), the end of the slope nearest the value.
fn inverse(c: &[(f64, f64)], value: f64) -> f64 {
    for w in c.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if y0 != y1 && (y0.min(y1)..=y0.max(y1)).contains(&value) {
            return x0 + (value - y0) / (y1 - y0) * (x1 - x0);
        }
    }
    let mut best = (f64::MAX, 0.0);
    for w in c.windows(2) {
        if w[0].1 != w[1].1 {
            for &(x, y) in w {
                let d = (y - value).abs();
                if d < best.0 {
                    best = (d, x);
                }
            }
        }
    }
    best.1
}

/// The MAF frequency, Hz, for which the firmware reports `g_per_s` [MED].
pub fn maf_hz(g_per_s: f64) -> f64 {
    inverse(MAF, g_per_s)
}

/// The input called `key`.
pub fn input(key: &str) -> Option<&'static Input> {
    INPUTS.iter().find(|i| i.key == key)
}

/// Values for the analog inputs, by key.
#[derive(Clone, Debug, PartialEq)]
pub struct Sensors {
    /// (key, value), in the units of the input's curve.
    pub values: Vec<(&'static str, f64)>,
}

impl Sensors {
    /// A warm engine, ignition on, not turning.
    pub fn ignition_on() -> Self {
        Self {
            values: vec![
                ("coolant", 77.0),
                ("intake", 26.0),
                ("intake2", 30.0), // [SUP]: 54 degC at idle, behind the hot intercooler
                ("fuel-temp", 28.0),
                ("fuel-temp2", 28.0), // [SUP]: as sensor 1
                ("baro", 98.0),
                ("boost", 98.0),   // [SUP]: the baro, nothing turning
                ("battery", 12.6), // [SUP]: a rested battery
                ("rail", 0.0),
                ("pedal1", 19.6),
                ("pedal2", 9.8), // [SUP]: half of sensor 1, as GM pedals are built
                ("fuel-level", 75.0),
                ("egr", 0.0),
                ("dpf", 0.0),
                ("egt1", 60.0), // [SUP]: a warm, stopped exhaust
                ("egt2", 60.0),
                ("flap", 0.0),
                ("clutch", 100.0), // [SUP]: the pedal released reads 100 % here
                ("ac-pressure", 700.0), // [SUP]
                ("ref1", 5.0),
                ("ref2", 5.0),
                ("oil-level", 0.0),
                ("o2-voltage", 2.5), // [SUP]: mid range
                ("o2-current", 0.0),
                ("scv-feedback", 0.0),
                (MAF_KEY, 0.0),
            ],
        }
    }

    /// The same engine idling at about 750 rpm.
    pub fn idle() -> Self {
        let mut s = Self::ignition_on();
        for (k, v) in [
            ("intake2", 54.0),
            ("boost", 97.7),
            ("battery", 14.0), // [SUP]: charging
            ("rail", 23.7),
            ("egr", 33.7),
            ("egt1", 199.5),
            ("egt2", 217.4),
            ("scv-feedback", 1729.0),
            (MAF_KEY, 5.47),
        ] {
            s.set(k, v).expect("known key");
        }
        s
    }

    /// Sets one input. Fails on an unknown key.
    pub fn set(&mut self, key: &str, value: f64) -> Result<(), String> {
        let key = known(key)?;
        match self.values.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.values.push((key, value)),
        }
        Ok(())
    }

    /// The MAF frequency on TI0C, Hz, if the MAF is set.
    pub fn maf_hz(&self) -> Option<f64> {
        self.values
            .iter()
            .find(|(k, _)| *k == MAF_KEY)
            .map(|&(_, v)| maf_hz(v))
    }

    /// The A/D codes: (channel, code) for every input set.
    pub fn codes(&self) -> Vec<(usize, u16)> {
        self.values
            .iter()
            .filter_map(|&(k, v)| input(k).map(|i| (i.channel, i.code(v))))
            .collect()
    }
}

/// The static key equal to `key`, or an error listing them all.
fn known(key: &str) -> Result<&'static str, String> {
    if key == MAF_KEY {
        return Ok(MAF_KEY);
    }
    input(key).map(|i| i.key).ok_or_else(|| {
        let keys: Vec<&str> = INPUTS.iter().map(|i| i.key).collect();
        format!(
            "no sensor '{key}'; there are: {}, {MAF_KEY}",
            keys.join(", ")
        )
    })
}

/// Parses `key=value` for a sensor.
pub fn parse(s: &str) -> Result<(&'static str, f64), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("--sensor wants key=value, got '{s}'"))?;
    let k = known(k)?;
    let v: f64 = v.parse().map_err(|_| format!("bad value in '{s}'"))?;
    Ok((k, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_read_back_as_the_values() {
        for (k, v) in Sensors::idle().values {
            if k == MAF_KEY {
                continue;
            }
            let i = input(k).unwrap();
            let back = i.value(i.code(v));
            let span = i.curve.iter().map(|p| p.1).fold(f64::MIN, f64::max)
                - i.curve.iter().map(|p| p.1).fold(f64::MAX, f64::min);
            assert!(
                (back - v).abs() <= span / 100.0,
                "{k}: {v} -> code {} -> {back}",
                i.code(v)
            );
        }
    }

    #[test]
    fn every_channel_once_and_known_curves() {
        let mut ch: Vec<usize> = INPUTS.iter().map(|i| i.channel).collect();
        ch.sort();
        ch.dedup();
        assert_eq!(ch.len(), INPUTS.len());
        // The coolant sensor is an NTC: the code falls as it warms.
        let c = input("coolant").unwrap();
        assert!(c.code(90.0) < c.code(20.0));
        // Battery: 40.5 mV a code, clamped at 20 V.
        let b = input("battery").unwrap();
        assert_eq!(b.code(14.0), 345);
        assert_eq!(b.code(30.0), b.code(20.0), "past the clamp, the clamp");
    }

    #[test]
    fn set_and_parse() {
        let mut s = Sensors::ignition_on();
        s.set("coolant", 20.0).unwrap();
        assert!(s.values.contains(&("coolant", 20.0)));
        assert!(s.set("nonsense", 1.0).is_err());
        assert_eq!(parse("rail=30").unwrap(), ("rail", 30.0));
        assert!(parse("rail").is_err());
        assert_eq!(parse("maf=5").unwrap(), ("maf", 5.0));
    }

    #[test]
    fn maf_frequencies() {
        // 0 g/s: the end of the flat part, where the slope starts.
        assert_eq!(maf_hz(0.0), 1750.0);
        let hz = maf_hz(23.45);
        assert!((hz - 3000.0).abs() < 1.0, "{hz}");
        assert!(Sensors::idle().maf_hz().unwrap() > 2000.0);
        assert!(Sensors { values: vec![] }.maf_hz().is_none());
    }
}
