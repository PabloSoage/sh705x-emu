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

/// The ABS: frame 0x1E9 every 21.4 ms, as the car sent it standing still [MED]. What
/// its bytes mean is not known [SUP].
pub fn abs() -> Periodic {
    let mut p = Periodic::default();
    p.add(
        Frame::standard(0x1E9, &[0x0F, 0x98, 0x00, 0x0C, 0x0F, 0xFA, 0x00, 0x00]),
        21.4,
        0,
    );
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
