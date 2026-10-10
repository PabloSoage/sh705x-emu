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
