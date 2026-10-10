//! Crankshaft and camshaft tooth wheels at a set engine speed, as edges on the ECU's
//! input pins.
//!
//! The crank wheel has `teeth` positions with `missing` of them left out, 60-4 by
//! default; each tooth present is a pulse, high for the first half of
//! its pitch and low for the second. The cam wheel gives `cam_teeth` pulses per two
//! crank revolutions at the listed crank angles. Both are ideal square waves of a
//! sensor already conditioned to logic levels: what the ECU's input capture sees.
//!
//! The crank wheel is 60-4 [DER]: the firmware's reference routine (0xE854, run at
//! the first tooth after the gap, which TCNT10G finds as an interval of over 2.5
//! pitches) wants 56 teeth counted on TCNT10B from one gap to the next, or drops the
//! phase; TCCLR10 = 7680 = 120 x 64 makes 60 positions to a revolution. With 60-2 it
//! counts 58 and never keeps the phase, so it never injects.
//!
//! Which pins the A17DTJ uses is not settled [SUP]: the firmware sets
//! TIOR0 to capture TI0A on falling and TI0B on rising edges, which fits a crank wheel
//! on TI0A and a cam on TI0B; and its tooth interrupt reads channel 10, whose TI10
//! input takes the crank signal too.

use sh7059::pins::{Edge, Pin, Stimulus};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// CPU cycles per second (φ = 80 MHz).
pub const CYCLES_PER_S: f64 = 80e6;

/// A crank and cam pair turning at a speed that can change from one tooth to the next.
#[derive(Clone, Debug)]
pub struct Wheels {
    /// Engine speed, rpm.
    pub rpm: f64,
    /// Tooth positions on the crank wheel.
    pub teeth: u32,
    /// Positions without a tooth, at the end of the revolution.
    pub missing: u32,
    /// Crank angles (degrees over 0..720) where a cam pulse starts.
    pub cam_angles: Vec<f64>,
    /// Width of a cam pulse, crank degrees.
    pub cam_width: f64,
    /// Pins the crank signal reaches.
    pub crank_pins: Vec<Pin>,
    /// Cam pin.
    pub cam_pin: Pin,
    /// The edges not taken yet, computed a tooth pitch at a time; earliest first.
    queue: BinaryHeap<Reverse<(u64, u8, bool)>>,
    /// Pitches already turned into edges since the engine started.
    pitches: u64,
    /// The stretch turning at the current speed: the cycle it started at, and how
    /// many pitches had been turned by then.
    seg_start: f64,
    seg_pitches: u64,
}

fn pin_code(p: Pin) -> u8 {
    match p {
        Pin::Ti0(k) => k,
        Pin::Ti10 => 10,
    }
}

fn pin_of(code: u8) -> Pin {
    if code == 10 {
        Pin::Ti10
    } else {
        Pin::Ti0(code)
    }
}

impl Wheels {
    /// A 60-4 crank on TI0A and TI10, and a cam on TI0B with four teeth 180° apart plus a
    /// sync tooth 30° (crank) before the first, at `rpm`, starting at cycle `start`.
    ///
    /// The firmware's cam routine (0xDD80) keeps the interval between cam edges and
    /// synchronises when one is shorter than half the one before: a cam wheel with an
    /// extra tooth close to another. Swept over the phase to the crank gap and the place
    /// of the extra tooth, at 1000 and 2500 rpm, the firmware reports the speed exactly
    /// every time when the extra tooth comes 15° to 45° before a main tooth, whatever
    /// the phase; 60° to 90° before, never; anywhere else, about half the time [MED].
    /// The real wheel's tooth positions are not known [SUP].
    pub fn new(rpm: f64, start: u64) -> Self {
        let mut w = Self {
            rpm: rpm.max(0.0),
            teeth: 60,
            missing: 4,
            cam_angles: vec![0.0, 180.0, 360.0, 540.0, 690.0],
            cam_width: 10.0,
            crank_pins: vec![Pin::Ti0(0), Pin::Ti10],
            cam_pin: Pin::Ti0(1),
            queue: BinaryHeap::new(),
            pitches: 0,
            seg_start: start as f64,
            seg_pitches: 0,
        };
        w.top_up();
        w
    }

    /// Degrees of crank between two tooth positions.
    fn pitch(&self) -> f64 {
        360.0 / self.teeth as f64
    }

    /// Cycles per crank degree at the current speed.
    fn cycles_per_degree(&self) -> f64 {
        CYCLES_PER_S * 60.0 / (self.rpm * 360.0)
    }

    /// The cycle at `deg` degrees into the current stretch.
    fn at(&self, deg: f64) -> f64 {
        self.seg_start + deg * self.cycles_per_degree()
    }

    /// The cycle where the pitches computed so far end.
    fn computed_end(&self) -> f64 {
        self.at((self.pitches - self.seg_pitches) as f64 * self.pitch())
    }

    /// Computes pitches until the earliest edge comes before the uncomputed part, so
    /// nothing computed later can come before it.
    fn top_up(&mut self) {
        if self.rpm == 0.0 {
            return;
        }
        while self
            .queue
            .peek()
            .is_none_or(|Reverse((t, _, _))| *t as f64 >= self.computed_end())
        {
            self.refill();
        }
    }

    /// Changes speed at cycle `now`: edges already computed (the current tooth pitch)
    /// stay, the rest follow the new speed, as a real crank's would. 0 stops the engine
    /// at once, with no more edges; from a stop, it starts turning at `now`, tooth 0
    /// first.
    pub fn set_rpm(&mut self, rpm: f64, now: u64) {
        if rpm <= 0.0 {
            self.rpm = 0.0;
            self.queue.clear();
            return;
        }
        if self.rpm == 0.0 {
            self.pitches = 0;
            self.seg_start = now as f64;
        } else {
            self.seg_start = self.computed_end();
        }
        self.seg_pitches = self.pitches;
        self.rpm = rpm;
        self.top_up();
    }

    /// Turns one more tooth pitch into edges.
    fn refill(&mut self) {
        let pitch = self.pitch();
        // Where this pitch is in the cam's 720° cycle, and in the current stretch.
        let k = self.pitches % (2 * self.teeth as u64);
        let a0 = k as f64 * pitch;
        let base = (self.pitches - self.seg_pitches) as f64 * pitch;
        let mut edges = Vec::new();
        if (k % self.teeth as u64) < (self.teeth - self.missing) as u64 {
            for &p in &self.crank_pins {
                edges.push((base, p, true));
                edges.push((base + pitch / 2.0, p, false));
            }
        }
        for &c in &self.cam_angles {
            for (e, high) in [(c, true), ((c + self.cam_width) % 720.0, false)] {
                if (a0..a0 + pitch).contains(&e) {
                    edges.push((base + e - a0, self.cam_pin, high));
                }
            }
        }
        for (deg, pin, high) in edges {
            let t = self.at(deg).round() as u64;
            self.queue.push(Reverse((t, pin_code(pin), high)));
        }
        self.pitches += 1;
    }
}

impl Stimulus for Wheels {
    fn peek(&self) -> Option<Edge> {
        self.queue.peek().map(|Reverse((at, k, high))| Edge {
            at: *at,
            pin: pin_of(*k),
            high: *high,
        })
    }

    fn take(&mut self) -> Option<Edge> {
        let e = self.peek();
        self.queue.pop();
        self.top_up();
        e
    }

    fn set(&mut self, key: &str, value: f64, now: u64) {
        if key == "rpm" {
            self.set_rpm(value, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_60_4_wheel_at_1000_rpm() {
        let mut w = Wheels::new(1000.0, 0);
        // 1000 rpm: 60 ms a revolution, 1 ms a tooth, 80 000 cycles.
        let crank: Vec<Edge> = std::iter::from_fn(|| w.take())
            .take(400)
            .filter(|e| e.pin == Pin::Ti0(0))
            .collect();
        assert!(crank.windows(2).all(|p| p[0].at <= p[1].at));
        let rising: Vec<u64> = crank.iter().filter(|e| e.high).map(|e| e.at).collect();
        assert_eq!(rising[0], 0);
        assert_eq!(rising[1] - rising[0], 80_000);
        // 56 teeth, then a gap of five pitches before tooth 0 of the next revolution.
        assert_eq!(rising[56] - rising[55], 5 * 80_000);
        assert_eq!(rising[56], 4_800_000, "one revolution: 60 ms");
        let falling = crank.iter().find(|e| !e.high).unwrap();
        assert_eq!(falling.at, 40_000, "half a pitch high");
    }

    #[test]
    fn stopping_and_starting() {
        let mut w = Wheels::new(0.0, 0);
        assert!(w.peek().is_none(), "0 rpm: still");
        w.set("rpm", 1000.0, 5000);
        assert_eq!(w.peek().map(|e| e.at), Some(5000), "tooth 0 at the start");
        w.take();
        w.set("rpm", 0.0, 6000);
        assert!(w.take().is_none(), "stopped: nothing more");
        w.set("other", 1.0, 7000);
        assert!(w.peek().is_none());
    }

    #[test]
    fn a_new_speed_takes_from_the_next_tooth() {
        // 1000 rpm: 80 000 cycles a pitch. Halved after the first tooth: the pitch in
        // hand keeps its speed, the next ones take twice as long.
        let mut w = Wheels::new(1000.0, 0);
        let first = w.take().unwrap();
        assert_eq!((first.at, first.high), (0, true));
        w.set("rpm", 500.0, 10);
        let rising: Vec<u64> = std::iter::from_fn(|| w.take())
            .filter(|e| e.pin == Pin::Ti0(0) && e.high)
            .take(3)
            .map(|e| e.at)
            .collect();
        assert_eq!(rising, [80_000, 240_000, 400_000]);
    }

    #[test]
    fn the_cam_turns_once_per_two_revolutions_and_edges_stay_in_order() {
        let mut w = Wheels::new(3000.0, 1000);
        let all: Vec<Edge> = std::iter::from_fn(|| w.take()).take(1000).collect();
        assert!(all.windows(2).all(|p| p[0].at <= p[1].at), "time order");
        let cam: Vec<u64> = all
            .iter()
            .filter(|e| e.pin == Pin::Ti0(1) && e.high)
            .map(|e| e.at)
            .collect();
        // 3000 rpm: 20 ms a revolution, 1.6 M cycles; five cam teeth per two
        // revolutions, the last 30° before the first of the next two.
        let at = |deg: f64| 1000 + (deg * 1_600_000.0 / 360.0).round() as u64;
        assert_eq!(
            &cam[..5],
            &[at(0.0), at(180.0), at(360.0), at(540.0), at(690.0)]
        );
        assert_eq!(cam[5] - cam[0], 3_200_000, "the pattern repeats every 720°");
        assert!(
            2 * (cam[5] - cam[4]) < cam[4] - cam[3],
            "the sync interval is under half the one before"
        );
    }
}
