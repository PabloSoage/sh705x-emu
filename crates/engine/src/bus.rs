//! Other nodes on the ECU's CAN bus: each sends its frames at a fixed period.
//!
//! The ECU expects frames from the rest of the car (ABS, body, instrument cluster…) and
//! sets "lost communication" codes (U01xx) when they stop. [`Periodic`] stands in for
//! those nodes with fixed frames at fixed periods; what each frame carries is up to the
//! caller.
//!
//! The firmware sets up receive mailboxes for 20 identifiers. Fed, one at a time, the
//! frames the car's other modules sent (scanmatik-bench captures, standing still), only
//! 0x1E9 clears U0121, "lost communication with the ABS" [MED]: [`abs`]. The pair
//! 0x683 (sent) / 0x680 (received) looks like the immobilizer's exchange with the body
//! module [SUP] and is left alone.

use sh7059::hcan::{CanSource, Frame};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// One periodic frame.
#[derive(Clone, Copy, Debug)]
pub struct Message {
    /// The frame.
    pub frame: Frame,
    /// Period, CPU cycles.
    pub period: u64,
    /// Cycle of the next one.
    pub next: u64,
}

/// Periodic frames from other nodes.
#[derive(Clone, Debug, Default)]
pub struct Periodic(pub Vec<Message>);

impl Periodic {
    /// Adds `frame` every `period_ms`, the first at cycle `first`.
    pub fn add(&mut self, frame: Frame, period_ms: f64, first: u64) {
        let period = (period_ms * crate::wheel::CYCLES_PER_S / 1000.0).max(1.0) as u64;
        self.0.push(Message {
            frame,
            period,
            next: first,
        });
    }

    fn first(&self) -> Option<usize> {
        self.0
            .iter()
            .enumerate()
            .min_by_key(|(_, m)| m.next)
            .map(|(k, _)| k)
    }
}

impl CanSource for Periodic {
    fn peek(&self) -> Option<(u64, Frame)> {
        self.first().map(|k| (self.0[k].next, self.0[k].frame))
    }

    fn take(&mut self) -> Option<(u64, Frame)> {
        let k = self.first()?;
        let m = &mut self.0[k];
        let out = (m.next, m.frame);
        m.next += m.period;
        Some(out)
    }
}

/// The road speed the wheels turn at, set from outside while they are on the bus.
#[derive(Clone, Debug, Default)]
pub struct Road(Arc<AtomicU64>);

impl Road {
    /// Sets the speed, km/h.
    pub fn set(&self, kmh: f64) {
        self.0.store(kmh.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// The speed, km/h.
    pub fn kmh(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// The speed at which a wheel gives a pulse a millisecond, km/h: the firmware's own scale
/// (its road speed, `FFFF17A8`, is 150 for 1 pulse/ms with stamps in microseconds, and
/// follows it exactly from 30 to 300) [MED]. It is 48 pulses a turn of a 2.0 m wheel.
const KMH_A_PULSE_A_MS: f64 = 150.0;

/// The ABS of a car on the road: the wheels' frames 0x0C1 and 0x0C5 (two wheels each: a
/// rolling count of two bits, the pulses counted, ten bits, and the instant of the last
/// one, 16 bits of microseconds) and 0x1E9 as [`abs`] sends it. The four wheels turn
/// alike. The layout is what the car's frames, all standing still, leave room for, and the
/// firmware reads a speed from it [DER].
#[derive(Clone, Debug)]
pub struct RollingWheels {
    road: Road,
    /// The next cycle of each: 0x0C1, 0x0C5, 0x1E9.
    next: [u64; 3],
    /// The rolling count of each wheels' frame.
    count: [u16; 2],
    /// Pulses so far, at `cycle`.
    pulses: f64,
    cycle: u64,
    /// The instant of the last whole pulse, µs.
    stamp: f64,
    /// The frame that comes next.
    ready: (u64, Frame),
}

/// Their periods, CPU cycles: the car's 10.7, 10.8 and 21.4 ms.
const ROLLING: [u64; 3] = [856_000, 864_000, 1_712_000];

impl RollingWheels {
    /// The wheels on `road`.
    pub fn new(road: Road) -> Self {
        let mut w = Self {
            road,
            next: [0, 1000, 2000],
            count: [0; 2],
            pulses: 0.0,
            cycle: 0,
            stamp: 0.0,
            ready: (0, Frame::standard(0x1E9, &[])),
        };
        w.prepare();
        w
    }

    /// Turns the wheels to the next frame and writes it.
    fn prepare(&mut self) {
        let k = (0..3).min_by_key(|&k| self.next[k]).unwrap_or(0);
        let at = self.next[k];
        self.next[k] += ROLLING[k];
        let per_ms = self.road.kmh() / KMH_A_PULSE_A_MS;
        let before = self.pulses.floor();
        self.pulses += per_ms * (at - self.cycle) as f64 / (crate::wheel::CYCLES_PER_S / 1000.0);
        self.cycle = at;
        let whole = self.pulses.floor();
        if whole > before {
            let us = at as f64 / (crate::wheel::CYCLES_PER_S / 1e6);
            self.stamp = us - (self.pulses - whole) / per_ms * 1000.0;
        }
        let frame = if k == 2 {
            Frame::standard(0x1E9, &ABS)
        } else {
            let count = self.count[k];
            self.count[k] = (count + 1) & 3;
            let [a, b] = (count << 12 | (whole as u64 & 0x3FF) as u16).to_be_bytes();
            let [c, d] = ((self.stamp as u64 & 0xFFFF) as u16).to_be_bytes();
            Frame::standard([0x0C1, 0x0C5][k], &[a, b, c, d, a, b, c, d])
        };
        self.ready = (at, frame);
    }
}

impl CanSource for RollingWheels {
    fn peek(&self) -> Option<(u64, Frame)> {
        Some(self.ready)
    }

    fn take(&mut self) -> Option<(u64, Frame)> {
        let out = self.ready;
        self.prepare();
        Some(out)
    }
}

/// What 0x1E9 carried standing still [MED]. What its bytes mean is not known [SUP].
const ABS: [u8; 8] = [0x0F, 0x98, 0x00, 0x0C, 0x0F, 0xFA, 0x00, 0x00];

/// The ABS: frame 0x1E9 every 21.4 ms, as the car sent it standing still [MED].
pub fn abs() -> Periodic {
    let mut p = Periodic::default();
    p.add(Frame::standard(0x1E9, &ABS), 21.4, 0);
    p
}

/// The car's other nodes as the ECU hears them standing still: for each identifier the
/// firmware listens to that the car sent (scanmatik-bench `60-identificar-app.log`), its
/// most frequent frame at its measured period [MED]. Left out: 0x4E1, which carries part
/// of the VIN, and 0x4E9, which may carry the odometer [SUP]; 0x680, the immobilizer's
/// side, is not sent by any of them. Frames with rolling counters are frozen at their most
/// frequent value [SUP].
pub fn car() -> Periodic {
    let mut p = Periodic::default();
    for (id, data, period_ms) in [
        (
            0x0C1,
            &[0x82, 0x21, 0x0A, 0x30, 0x82, 0x35, 0x94, 0xF4][..],
            10.7,
        ),
        (
            0x0C5,
            &[0xC1, 0xCA, 0x9C, 0x04, 0x41, 0xEA, 0x80, 0xF8][..],
            10.8,
        ),
        (0x0F1, &[0x34, 0x04, 0x00, 0x40][..], 10.7),
        (0x1C7, &[0x06, 0xA0, 0x39, 0x60, 0x03, 0xFF, 0x3F][..], 21.4),
        (0x1E1, &[0x00, 0xFD, 0x07, 0x00][..], 32.2),
        (
            0x1E5,
            &[0x46, 0x01, 0x59, 0xC0, 0x00, 0xFE, 0xA5, 0x00][..],
            10.8,
        ),
        (
            0x1E9,
            &[0x0F, 0x98, 0x00, 0x0C, 0x0F, 0xFA, 0x00, 0x00][..],
            21.4,
        ),
        (
            0x1F1,
            &[0xAE, 0x0A, 0x00, 0x00, 0x10, 0x00, 0x00, 0x72][..],
            43.4,
        ),
        (0x1F3, &[0x40, 0xFC, 0x00][..], 32.5),
        (0x2F9, &[0xE8, 0x00, 0x00, 0x00, 0x00, 0x00][..], 53.6),
        (
            0x3C9,
            &[0x07, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00][..],
            108.2,
        ),
        (
            0x3F1,
            &[0x00, 0xC1, 0x96, 0x08, 0x00, 0xFA, 0x02, 0x75][..],
            272.7,
        ),
    ] {
        p.add(Frame::standard(id, data), period_ms, 0);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames come in time order, the count of each rolls over, and the pulses and their
    /// stamps say the speed they were given.
    #[test]
    fn the_rolling_wheels_say_their_speed() {
        let road = Road::default();
        road.set(90.0);
        let mut w = RollingWheels::new(road.clone());
        let (mut last, mut seen) = (0, Vec::new());
        for _ in 0..200 {
            let (at, f) = w.take().unwrap();
            assert!(at >= last);
            last = at;
            if f.id == 0x0C1 {
                let word = |k: usize| u32::from(f.data[k]) << 8 | u32::from(f.data[k + 1]);
                seen.push((word(0) >> 12, word(0) & 0x3FF, word(2)));
            }
        }
        for p in seen.windows(2).skip(1) {
            assert_eq!(p[1].0, (p[0].0 + 1) & 3);
            let pulses = (p[1].1 + 0x400 - p[0].1) & 0x3FF;
            let us = (p[1].2 + 0x1_0000 - p[0].2) & 0xFFFF;
            let kmh = f64::from(pulses) / f64::from(us) * 1000.0 * KMH_A_PULSE_A_MS;
            assert!((kmh - 90.0).abs() < 0.1, "{kmh}");
        }
        road.set(0.0);
        assert_eq!(road.kmh(), 0.0);
    }

    #[test]
    fn frames_come_in_time_order_at_their_periods() {
        let mut p = Periodic::default();
        p.add(Frame::standard(0x0C1, &[1]), 10.0, 0);
        p.add(Frame::standard(0x1F1, &[2]), 25.0, 1000);
        let got: Vec<(u64, u32)> = std::iter::from_fn(|| p.take())
            .take(6)
            .map(|(t, f)| (t, f.id))
            .collect();
        // 10 ms at 80 MHz is 800 000 cycles.
        assert_eq!(
            got,
            [
                (0, 0x0C1),
                (1000, 0x1F1),
                (800_000, 0x0C1),
                (1_600_000, 0x0C1),
                (2_001_000, 0x1F1),
                (2_400_000, 0x0C1)
            ]
        );
    }

    #[test]
    fn the_car_set_keeps_the_vin_and_the_immobilizer_out() {
        let p = car();
        let ids: Vec<u32> = p.0.iter().map(|m| m.frame.id).collect();
        assert_eq!(ids.len(), 12);
        assert!(ids.contains(&0x1E9), "the ABS, whose absence sets U0121");
        for out in [0x4E1, 0x4E9, 0x680] {
            assert!(!ids.contains(&out), "{out:03X}");
        }
    }
}
