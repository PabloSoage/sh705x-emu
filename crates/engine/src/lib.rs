//! The simulated engine that closes the loop with the emulated ECU.
//!
//! So far: the crankshaft and camshaft wheels at a set speed, as edges on the ATU-II
//! input captures ([`wheel`]), and the analog sensors as A/D codes from physical values
//! ([`sensors`]), and other nodes on the CAN bus sending their periodic frames
//! ([`bus`]). Planned: a mean-value engine model driven by the injection, EGR,
//! boost and rail-pressure actuators.

pub mod bus;
pub mod curves;
pub mod sensors;
pub mod wheel;
