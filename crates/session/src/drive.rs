//! A recorded drive put on the ECU's inputs: engine speed, road speed (the wheels' frames
//! on the bus), pedal and temperatures as the car had them, moment by moment. What the
//! firmware asks for, and where its actuators go, is left to it and the plants.
//!
//! A drive is a CSV file with a header. `t` (s) and `rpm` are needed; `kmh` moves the
//! wheels; any sensor key (`pedal`, `coolant`, `intake`, `baro`...) sets that sensor; and
//! `recorded_egr` (%) and `recorded_air` (g/s) are what the car did, only to be shown.
//! Other columns are ignored.

use engine::bus::Road;
use engine::sensors::{MAF_KEY, PEDAL_KEY, input, pedal_sensors};
use engine::wheel::CYCLES_PER_S;

/// What is shown of the car's own recording: its name, its column, and the trace
/// ([`crate::scope::TRACES`]) it goes with.
pub const RECORDED: [(&str, &str, usize); 2] = [
    ("the car's EGR position", "recorded_egr", 2),
    ("the car's fresh air", "recorded_air", 12),
];

/// The sensors the plants drive: a drive does not set them.
const OF_THE_PLANTS: [&str; 5] = ["scv-feedback", "egr", "boost", "rail", MAF_KEY];

/// One moment of a drive.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    /// When, s from its start.
    t: f64,
    rpm: f64,
    kmh: f64,
    recorded: [f32; RECORDED.len()],
    /// Sensors by key.
    sensors: Vec<(String, f64)>,
}

/// A recorded drive.
#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    rows: Vec<Row>,
}

impl Drive {
    /// Reads a drive from its CSV text.
    pub fn read(text: &str) -> Result<Self, String> {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let head: Vec<&str> = lines
            .next()
            .ok_or("the drive is empty")?
            .split(',')
            .map(str::trim)
            .collect();
        let col = |n: &str| head.iter().position(|h| *h == n);
        let t = col("t").ok_or("the drive has no column 't'")?;
        let rpm = col("rpm").ok_or("the drive has no column 'rpm'")?;
        let kmh = col("kmh");
        let recorded = RECORDED.map(|(_, c, _)| col(c));
        let sensors: Vec<(usize, &str)> = head
            .iter()
            .enumerate()
            .filter(|(_, k)| **k == PEDAL_KEY || input(k).is_some())
            .filter(|(_, k)| !OF_THE_PLANTS.contains(k))
            .map(|(i, k)| (i, *k))
            .collect();
        let mut rows: Vec<Row> = Vec::new();
        for (n, line) in lines.enumerate() {
            let f: Vec<&str> = line.split(',').map(str::trim).collect();
            let get = |i: usize| {
                f.get(i)
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("the drive's row {}: no '{}'", n + 1, head[i]))
            };
            let row = Row {
                t: get(t)?,
                rpm: get(rpm)?.clamp(0.0, 8000.0),
                kmh: kmh.map_or(Ok(0.0), get)?.clamp(0.0, 400.0),
                recorded: recorded
                    .map(|c| c.and_then(|c| get(c).ok()).map_or(f32::NAN, |v| v as f32)),
                sensors: sensors
                    .iter()
                    .map(|&(i, k)| Ok((k.to_string(), get(i)?)))
                    .collect::<Result<_, String>>()?,
            };
            if rows.last().is_some_and(|p| p.t >= row.t) {
                return Err(format!("the drive's row {}: 't' does not rise", n + 1));
            }
            rows.push(row);
        }
        if rows.is_empty() {
            return Err("the drive has no rows".into());
        }
        Ok(Self { rows })
    }

    /// How long it lasts, s.
    pub fn seconds(&self) -> f64 {
        self.rows[self.rows.len() - 1].t - self.rows[0].t
    }
}

/// Where a drive is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Moment {
    /// s from its start; negative while the ECU boots at its first moment.
    pub t: f64,
    /// Its length, s.
    pub seconds: f64,
    /// Road speed, km/h.
    pub kmh: f64,
    /// Pedal travel, %, if the drive has it.
    pub pedal: Option<f64>,
    /// What the car did ([`RECORDED`]); NaN where the drive does not say.
    pub recorded: [f32; RECORDED.len()],
}

/// ECU time at the drive's first moment before it starts, s: the firmware boots, and its
/// loops settle.
pub const WARM_UP: f64 = 4.0;

/// A drive on its way.
pub(crate) struct Driving {
    drive: Drive,
    road: Road,
    /// Cycle of its start.
    start: u64,
    /// The row in force.
    at: usize,
}

impl Driving {
    /// `drive`, started [`WARM_UP`] after cycle `now`, with its wheels on `road`.
    pub fn new(drive: Drive, road: Road, now: u64) -> Self {
        road.set(drive.rows[0].kmh);
        Self {
            drive,
            road,
            start: now + (WARM_UP * CYCLES_PER_S) as u64,
            at: 0,
        }
    }

    fn seconds_at(&self, cycle: u64) -> f64 {
        (cycle as f64 - self.start as f64) / CYCLES_PER_S
    }

    /// The row in force moved on to `cycle`; whether it changed.
    pub fn advance(&mut self, cycle: u64) -> bool {
        let was = self.at;
        let t = self.seconds_at(cycle) + self.drive.rows[0].t;
        while self.drive.rows.get(self.at + 1).is_some_and(|r| r.t <= t) {
            self.at += 1;
        }
        self.road.set(self.drive.rows[self.at].kmh);
        self.at != was
    }

    /// Engine speed now, rpm.
    pub fn rpm(&self) -> f64 {
        self.drive.rows[self.at].rpm
    }

    /// The analog sensors now, each as its own input's key and value.
    pub fn sensors(&self) -> Vec<(&'static str, f64)> {
        let mut out = Vec::new();
        let mut put = |k: &str, v: f64| {
            if let Some(i) = input(k) {
                out.push((i.key, v));
            }
        };
        for (k, v) in &self.drive.rows[self.at].sensors {
            if k == PEDAL_KEY {
                let (s1, s2) = pedal_sensors(*v);
                put("pedal1", s1);
                put("pedal2", s2);
            } else {
                put(k, *v);
            }
        }
        out
    }

    /// Where it is at `cycle`.
    pub fn moment(&self, cycle: u64) -> Moment {
        let r = &self.drive.rows[self.at];
        Moment {
            t: self.seconds_at(cycle),
            seconds: self.drive.seconds(),
            kmh: r.kmh,
            pedal: r
                .sensors
                .iter()
                .find(|(k, _)| k == PEDAL_KEY)
                .map(|&(_, v)| v),
            recorded: r.recorded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "t,rpm,kmh,pedal,coolant,maf,other,recorded_egr\n\
                       10.0,750,0,0,80,9,x,33\n10.1,900,3,12.5,80,9,x,\n10.3,1000,5,0,81,9,x,0\n";

    #[test]
    fn it_reads_what_it_drives_and_what_the_car_did() {
        let d = Drive::read(CSV).unwrap();
        assert!((d.seconds() - 0.3).abs() < 1e-9);
        let keys: Vec<&str> = d.rows[0].sensors.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["pedal", "coolant"], "the plants drive the MAF");
        assert_eq!(d.rows[0].recorded[0], 33.0);
        assert!(d.rows[1].recorded[0].is_nan() && d.rows[0].recorded[1].is_nan());
        for bad in [
            "",
            "t,kmh\n1,2\n",
            "t,rpm\n",
            "t,rpm\n1,x\n",
            "t,rpm\n2,1\n2,1\n",
        ] {
            assert!(Drive::read(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn it_goes_on_with_the_ecu_s_time_and_stays_at_its_end() {
        let road = Road::default();
        let mut d = Driving::new(Drive::read(CSV).unwrap(), road.clone(), 0);
        let at = |s: f64| ((WARM_UP + s) * CYCLES_PER_S) as u64;
        assert!(!d.advance(at(-1.0)) && d.rpm() == 750.0);
        assert!(d.advance(at(0.15)) && d.rpm() == 900.0 && road.kmh() == 3.0);
        assert_eq!(d.moment(at(0.15)).pedal, Some(12.5));
        let (s1, _) = pedal_sensors(12.5);
        assert!(d.sensors().contains(&("pedal1", s1)));
        assert!(d.advance(at(99.0)) && d.rpm() == 1000.0);
        assert!(!d.advance(at(100.0)));
    }
}
