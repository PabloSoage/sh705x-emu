//! The simulated engine that closes the loop with the emulated ECU.
//!
//! So far: the crankshaft and camshaft wheels, as edges on the ATU-II input captures, at
//! a speed that can change from one tooth to the next ([`wheel`]); the crank as a
//! rotating mass under the firmware's torque ([`crank`]); the air it draws ([`air`]); the
//! analog sensors as A/D codes from physical values ([`sensors`]); the SCV and EGR
//! actuators ([`actuators`]); the turbocharger ([`turbo`]); and other nodes on the CAN
//! bus sending their periodic frames ([`bus`]). Planned: the rail.

pub mod actuators;
pub mod air;
pub mod bus;
pub mod crank;
pub mod curves;
pub mod sensors;
pub mod turbo;
pub mod wheel;
