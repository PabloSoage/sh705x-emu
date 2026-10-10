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
}
