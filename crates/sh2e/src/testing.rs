//! A flat big-endian memory for tests, and a small harness to run instructions.

use crate::{Bus, Cpu};

/// Flat memory from address 0, plus a window at the top of the address space
/// (`0xFFFF_0000..`) for stacks.
pub struct Flat {
    low: Vec<u8>,
    high: Vec<u8>,
}

const HIGH: u32 = 0xFFFF_0000;

impl Flat {
    pub fn new(size: usize) -> Self {
        Self {
            low: vec![0; size],
            high: vec![0; 0x1_0000],
        }
    }

    fn slot(&mut self, addr: u32, n: usize) -> &mut [u8] {
        if addr >= HIGH {
            let a = (addr - HIGH) as usize;
            &mut self.high[a..a + n]
        } else {
            let a = addr as usize;
            &mut self.low[a..a + n]
        }
    }
}

impl Bus for Flat {
    fn read8(&mut self, addr: u32) -> u8 {
        self.slot(addr, 1)[0]
    }
    fn read16(&mut self, addr: u32) -> u16 {
        u16::from_be_bytes(self.slot(addr, 2).try_into().unwrap())
    }
    fn read32(&mut self, addr: u32) -> u32 {
        u32::from_be_bytes(self.slot(addr, 4).try_into().unwrap())
    }
    fn write8(&mut self, addr: u32, value: u8) {
        self.slot(addr, 1)[0] = value;
    }
    fn write16(&mut self, addr: u32, value: u16) {
        self.slot(addr, 2).copy_from_slice(&value.to_be_bytes());
    }
    fn write32(&mut self, addr: u32, value: u32) {
        self.slot(addr, 4).copy_from_slice(&value.to_be_bytes());
    }
}

/// Where test programs start.
pub const CODE: u32 = 0x1000;
/// Initial stack pointer in tests.
pub const STACK: u32 = 0xFFFF_8000;
/// VBR in tests: the vector table sits at 0x400, each vector pointing at 0x3000 + 4 × n.
pub const VBR: u32 = 0x400;

/// Where the handler for `vector` starts in tests.
pub fn handler(vector: u32) -> u32 {
    0x3000 + 4 * vector
}

/// A core and memory with `code` at [`CODE`], ready to step.
pub fn machine(code: &[u16]) -> (Cpu, Flat) {
    let mut mem = Flat::new(0x1_0000);
    for (i, c) in code.iter().enumerate() {
        mem.write16(CODE + 2 * i as u32, *c);
    }
    for v in 0..256 {
        mem.write32(VBR + 4 * v, handler(v));
    }
    let mut cpu = Cpu::new();
    cpu.pc = CODE;
    cpu.r[15] = STACK;
    cpu.vbr = VBR;
    cpu.fpscr = crate::FPSCR_RESET;
    (cpu, mem)
}
