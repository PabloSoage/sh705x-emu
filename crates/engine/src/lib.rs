//! The simulated engine that closes the loop with the emulated ECU.
//!
//! So far: the crankshaft and camshaft wheels at a set speed, as edges on the ATU-II
//! input captures ([`wheel`]). Planned (phase 5): sensor voltages into the A/D
//! converters and a mean-value engine model driven by the injection, EGR, boost and
//! rail-pressure actuators.

pub mod wheel;
