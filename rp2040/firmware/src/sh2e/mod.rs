//! Núcleo de CPU SH-2E portado a bare-metal `#![no_std]` para RP2040.

mod bus;
mod cpu;
pub mod decode;
mod exec;
mod fpu;

pub use bus::Bus;
pub use cpu::{
    Cpu, FPSCR_RESET, SR_I_MASK, SR_M, SR_MASK, SR_Q, SR_S, SR_T, Unverified, Vector, vector,
};
pub use exec::{Decoded, predecode};
pub use fpu::{FPSCR_FIXED, FPSCR_WRITABLE, QNAN};
