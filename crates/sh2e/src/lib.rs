//! SH-2E CPU core.
//!
//! The SH-2E is the SH-2 integer core plus a single-precision FPU that rounds toward
//! zero only. Reference: Renesas REJ09B0177-0500 (SH7059/SH7058S hardware manual),
//! sections 2 (CPU), 3 (FPU) and 6 (exception processing).
//!
//! The core talks to the rest of the chip through the [`Bus`] trait, so the same core
//! can run against the full SH7059 model or against a flat memory in tests.

mod bus;
mod cpu;

pub use bus::Bus;
pub use cpu::{Cpu, FPSCR_RESET, SR_I_MASK, Vector};
