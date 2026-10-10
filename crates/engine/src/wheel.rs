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
    /// A 60-2 crank on TI0A with one cam pulse per cycle on TI0B, at `rpm`, starting at
    /// cycle `start`.
    pub fn new(rpm: f64, start: u64) -> Self {
        let mut w = Self {
            rpm: rpm.max(1.0),
            teeth: 60,
            missing: 2,
            cam_angles: vec![90.0],
            cam_width: 30.0,
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

    /// Changes speed: edges already computed (the current revolution) stay, the rest
    /// follow the new speed.
    pub fn set_rpm(&mut self, rpm: f64) {
        self.rpm = rpm.max(1.0);
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
    fn the_cam_turns_once_per_two_revolutions_and_edges_stay_in_order() {
        let mut w = Wheels::new(3000.0, 1000);
        let all: Vec<Edge> = std::iter::from_fn(|| w.take()).take(1000).collect();
        assert!(all.windows(2).all(|p| p[0].at <= p[1].at), "time order");
        let cam: Vec<&Edge> = all
            .iter()
            .filter(|e| e.pin == Pin::Ti0(1) && e.high)
            .collect();
        // 3000 rpm: 20 ms a revolution, 1.6 M cycles; a cam pulse every two.
        assert!(cam.len() >= 2);
        assert_eq!(cam[1].at - cam[0].at, 3_200_000);
        assert_eq!(cam[0].at, 1000 + (90.0 * 1_600_000.0 / 360.0) as u64);
    }
}
