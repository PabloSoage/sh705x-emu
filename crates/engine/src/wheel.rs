//! Crankshaft and camshaft tooth wheels at a set engine speed, as edges on the ECU's
//! input pins.
//!
//! The crank wheel has `teeth` positions with `missing` of them left out, the usual
//! 60-2 being the default; each tooth present is a pulse, high for the first half of
//! its pitch and low for the second. The cam wheel gives `cam_teeth` pulses per two
//! crank revolutions at the listed crank angles. Both are ideal square waves of a
//! sensor already conditioned to logic levels: what the ECU's input capture sees.
//!
//! Which pins and which wheel the A17DTJ has is not settled [SUP]: the firmware sets
//! TIOR0 to capture TI0A on falling and TI0B on rising edges, which fits a crank wheel
//! on TI0A and a cam on TI0B; and its tooth interrupt reads channel 10, whose TI10
//! input takes the crank signal too.

use sh7059::pins::{Edge, Pin, Stimulus};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// CPU cycles per second (φ = 80 MHz).
pub const CYCLES_PER_S: f64 = 80e6;

/// A crank and cam pair turning at a constant speed.
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
    /// The edges not taken yet, computed a revolution at a time; earliest first.
    queue: BinaryHeap<Reverse<(u64, u8, bool)>>,
    /// Revolutions (of 360°) already turned into edges.
    revs: u64,
    /// Cycle at which the next revolution to compute starts.
    rev_end: f64,
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
    /// A 60-2 crank on TI0A and TI10, and a cam on TI0B with four teeth 180° apart plus a
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
            missing: 2,
            cam_angles: vec![0.0, 180.0, 360.0, 540.0, 690.0],
            cam_width: 10.0,
            crank_pins: vec![Pin::Ti0(0), Pin::Ti10],
            cam_pin: Pin::Ti0(1),
            queue: BinaryHeap::new(),
            revs: 0,
            rev_end: start as f64,
        };
        w.top_up();
        w
    }

    /// Computes revolutions until the earliest edge comes before the uncomputed part, so
    /// nothing computed later can come before it.
    fn top_up(&mut self) {
        if self.rpm == 0.0 {
            return;
        }
        while self
            .queue
            .peek()
            .is_none_or(|Reverse((t, _, _))| *t as f64 >= self.rev_end)
        {
            self.refill();
        }
    }

    /// Cycles per crank degree at the current speed.
    fn cycles_per_degree(&self) -> f64 {
        CYCLES_PER_S * 60.0 / (self.rpm * 360.0)
    }

    /// Changes speed at cycle `now`: edges already computed (the current revolution)
    /// stay, the rest follow the new speed. 0 stops the engine at once, with no more
    /// edges; from a stop, it starts turning at `now`, tooth 0 first.
    pub fn set_rpm(&mut self, rpm: f64, now: u64) {
        if rpm <= 0.0 {
            self.rpm = 0.0;
            self.queue.clear();
            return;
        }
        let was_stopped = self.rpm == 0.0;
        self.rpm = rpm;
        if was_stopped {
            self.rev_end = now as f64;
            self.revs = 0;
            self.top_up();
        }
    }

    /// Turns one more revolution into edges.
    fn refill(&mut self) {
        let cpd = self.cycles_per_degree();
        let rev_start = self.rev_end;
        let pitch = 360.0 / self.teeth as f64;
        let mut edges = Vec::new();
        for k in 0..self.teeth - self.missing {
            let a = k as f64 * pitch;
            for &p in &self.crank_pins {
                edges.push((rev_start + a * cpd, p, true));
                edges.push((rev_start + (a + pitch / 2.0) * cpd, p, false));
            }
        }
        // The cam turns once per two revolutions: this revolution covers 0..360 or
        // 360..720 of its cycle.
        let base = if self.revs % 2 == 0 { 0.0 } else { 360.0 };
        for &c in &self.cam_angles {
            if (base..base + 360.0).contains(&c) {
                let a = c - base;
                edges.push((rev_start + a * cpd, self.cam_pin, true));
                edges.push((rev_start + (a + self.cam_width) * cpd, self.cam_pin, false));
            }
        }
        for (t, pin, high) in edges {
            self.queue.push(Reverse((t as u64, pin_code(pin), high)));
        }
        self.rev_end = rev_start + 360.0 * cpd;
        self.revs += 1;
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
    fn a_60_2_wheel_at_1000_rpm() {
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
        // 58 teeth, then a gap of three pitches before tooth 0 of the next revolution.
        assert_eq!(rising[58] - rising[57], 3 * 80_000);
        assert_eq!(rising[58], 4_800_000, "one revolution: 60 ms");
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
        let at = |deg: f64| 1000 + (deg * 1_600_000.0 / 360.0) as u64;
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
