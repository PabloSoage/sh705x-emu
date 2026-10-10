//! Signals into the chip's input pins from outside (the simulated engine), as edges in
//! time order. The chip takes each one at its exact cycle, between instructions or
//! while the CPU sleeps, like any other event.

/// An input pin that something outside drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pin {
    /// TI0A–TI0D (k = 0..=3): the ATU-II channel 0 input captures ICR0A–ICR0D.
    Ti0(u8),
    /// TI10: the ATU-II channel 10 tooth input (AGCK).
    Ti10,
}

/// One edge: when, where, and the level after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    /// CPU cycle.
    pub at: u64,
    /// The pin.
    pub pin: Pin,
    /// The level after the edge: true for a rising edge.
    pub high: bool,
}

/// Something that drives input pins: the engine, a test bench.
pub trait Stimulus: Send {
    /// The next edge, if any, without taking it.
    fn peek(&self) -> Option<Edge>;
    /// Takes the next edge.
    fn take(&mut self) -> Option<Edge>;
}

/// Edges given in advance, for tests.
#[derive(Clone, Debug, Default)]
pub struct Script(pub std::collections::VecDeque<Edge>);

impl Stimulus for Script {
    fn peek(&self) -> Option<Edge> {
        self.0.front().copied()
    }

    fn take(&mut self) -> Option<Edge> {
        self.0.pop_front()
    }
}

/// Several stimuli driving different pins, merged in time order.
#[derive(Default)]
pub struct Merge(pub Vec<Box<dyn Stimulus>>);

impl Merge {
    /// The source with the earliest edge.
    fn first(&self) -> Option<usize> {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(k, s)| s.peek().map(|e| (e.at, k)))
            .min()
            .map(|(_, k)| k)
    }
}

impl Stimulus for Merge {
    fn peek(&self) -> Option<Edge> {
        self.first().and_then(|k| self.0[k].peek())
    }

    fn take(&mut self) -> Option<Edge> {
        let k = self.first()?;
        self.0[k].take()
    }
}

/// A square wave on one pin, at a frequency that can change while it runs: what a
/// frequency-output sensor (a mass air flow meter) gives.
#[derive(Clone, Debug)]
pub struct SquareWave {
    /// The pin.
    pub pin: Pin,
    /// CPU cycles per second.
    pub clock: f64,
    hz: f64,
    /// Cycle of the next edge, kept fractional so the period does not drift.
    next: f64,
    high: bool,
}

impl SquareWave {
    /// A wave at `hz` starting with a rising edge at cycle `start`. A frequency of 0 or
    /// less gives no edges.
    pub fn new(pin: Pin, hz: f64, clock: f64, start: u64) -> Self {
        Self {
            pin,
            clock,
            hz,
            next: start as f64,
            high: false,
        }
    }

    /// Changes the frequency from the next edge on.
    pub fn set_hz(&mut self, hz: f64) {
        self.hz = hz;
    }
}

impl Stimulus for SquareWave {
    fn peek(&self) -> Option<Edge> {
        (self.hz > 0.0).then_some(Edge {
            at: self.next as u64,
            pin: self.pin,
            high: !self.high,
        })
    }

    fn take(&mut self) -> Option<Edge> {
        let e = self.peek()?;
        self.high = !self.high;
        self.next += self.clock / self.hz / 2.0;
        Some(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_wave_merged_with_a_script() {
        let wave = SquareWave::new(Pin::Ti0(2), 1000.0, 80e6, 0);
        let script = Script(
            [Edge {
                at: 50_000,
                pin: Pin::Ti10,
                high: true,
            }]
            .into_iter()
            .collect(),
        );
        let mut m = Merge(vec![Box::new(wave), Box::new(script)]);
        let edges: Vec<Edge> = std::iter::from_fn(|| m.take()).take(5).collect();
        let at: Vec<(u64, bool)> = edges.iter().map(|e| (e.at, e.high)).collect();
        // 1 kHz at 80 MHz: an edge every 40 000 cycles, rising first.
        assert_eq!(
            at,
            [
                (0, true),
                (40_000, false),
                (50_000, true),
                (80_000, true),
                (120_000, false)
            ]
        );
        assert_eq!(edges[2].pin, Pin::Ti10);
        assert!(SquareWave::new(Pin::Ti0(2), 0.0, 80e6, 0).peek().is_none());
    }
}
