use crate::Bus;

/// Interrupt mask bits I3..I0 of SR.
pub const SR_I_MASK: u32 = 0x0000_00F0;

/// FPSCR after a reset (manual table 2.1).
pub const FPSCR_RESET: u32 = 0x0004_0001;

/// Entries of the exception vector table used at reset (manual section 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vector {
    /// Vector 0: initial PC after a power-on reset.
    PowerOnPc = 0,
    /// Vector 1: initial SP after a power-on reset.
    PowerOnSp = 1,
    /// Vector 2: initial PC after a manual reset.
    ManualPc = 2,
    /// Vector 3: initial SP after a manual reset.
    ManualSp = 3,
}

impl Vector {
    /// Address of the vector in the table, which sits at address 0 after a reset
    /// (VBR = 0).
    pub fn address(self) -> u32 {
        self as u32 * 4
    }
}

/// Architectural state of an SH-2E.
///
/// Floating-point registers hold raw IEEE-754 bit patterns, not `f32`: the core does its
/// own rounding (toward zero) and must not let the host FPU touch the values on the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpu {
    /// General registers R0..R15; R15 is the stack pointer.
    pub r: [u32; 16],
    /// Status register.
    pub sr: u32,
    /// Global base register.
    pub gbr: u32,
    /// Vector base register.
    pub vbr: u32,
    /// Multiply-and-accumulate register, high.
    pub mach: u32,
    /// Multiply-and-accumulate register, low.
    pub macl: u32,
    /// Procedure register (return address).
    pub pr: u32,
    /// Program counter.
    pub pc: u32,
    /// Floating-point registers FR0..FR15, as raw bits.
    pub fr: [u32; 16],
    /// Floating-point communication register.
    pub fpul: u32,
    /// Floating-point status and control register.
    pub fpscr: u32,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    /// A core with every register at zero. Call [`Cpu::power_on_reset`] before running.
    pub fn new() -> Self {
        Self {
            r: [0; 16],
            sr: 0,
            gbr: 0,
            vbr: 0,
            mach: 0,
            macl: 0,
            pr: 0,
            pc: 0,
            fr: [0; 16],
            fpul: 0,
            fpscr: 0,
        }
    }

    /// Power-on reset, as in manual table 2.1: PC and R15 from vectors 0 and 1,
    /// SR.I = 0xF, VBR = 0 and FPSCR = H'00040001.
    ///
    /// The manual leaves every other register undefined. Here they are zero, so that a run
    /// is reproducible; firmware that reads one before writing it is a finding to report.
    pub fn power_on_reset<B: Bus>(&mut self, bus: &mut B) {
        *self = Self::new();
        self.pc = bus.read32(Vector::PowerOnPc.address());
        self.r[15] = bus.read32(Vector::PowerOnSp.address());
        self.sr = SR_I_MASK;
        self.vbr = 0;
        self.fpscr = FPSCR_RESET;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat big-endian memory starting at address 0.
    struct Flat(Vec<u8>);

    impl Bus for Flat {
        fn read8(&mut self, addr: u32) -> u8 {
            self.0[addr as usize]
        }
        fn read16(&mut self, addr: u32) -> u16 {
            let a = addr as usize;
            u16::from_be_bytes([self.0[a], self.0[a + 1]])
        }
        fn read32(&mut self, addr: u32) -> u32 {
            let a = addr as usize;
            u32::from_be_bytes(self.0[a..a + 4].try_into().unwrap())
        }
        fn write8(&mut self, addr: u32, value: u8) {
            self.0[addr as usize] = value;
        }
        fn write16(&mut self, addr: u32, value: u16) {
            let a = addr as usize;
            self.0[a..a + 2].copy_from_slice(&value.to_be_bytes());
        }
        fn write32(&mut self, addr: u32, value: u32) {
            let a = addr as usize;
            self.0[a..a + 4].copy_from_slice(&value.to_be_bytes());
        }
    }

    fn table(pc: u32, sp: u32) -> Flat {
        let mut m = Flat(vec![0; 64]);
        m.write32(0, pc);
        m.write32(4, sp);
        m.write32(8, 0xDEAD_BEEF);
        m.write32(12, 0xCAFE_F00D);
        m
    }

    #[test]
    fn power_on_reset_takes_pc_and_sp_from_vectors_0_and_1() {
        let mut cpu = Cpu::new();
        cpu.power_on_reset(&mut table(0x0000_1234, 0x8765_4320));
        assert_eq!(cpu.pc, 0x0000_1234);
        assert_eq!(cpu.r[15], 0x8765_4320);
    }

    #[test]
    fn power_on_reset_sets_the_documented_initial_values() {
        let mut cpu = Cpu::new();
        cpu.sr = 0xFFFF_FFFF;
        cpu.vbr = 0x0010_0000;
        cpu.r[3] = 7;
        cpu.power_on_reset(&mut table(0x100, 0x200));
        assert_eq!(cpu.sr, SR_I_MASK);
        assert_eq!(cpu.vbr, 0);
        assert_eq!(cpu.fpscr, 0x0004_0001);
        assert_eq!(cpu.r[3], 0, "undefined registers start at zero");
    }

    #[test]
    fn vector_addresses() {
        assert_eq!(Vector::PowerOnPc.address(), 0);
        assert_eq!(Vector::PowerOnSp.address(), 4);
        assert_eq!(Vector::ManualPc.address(), 8);
        assert_eq!(Vector::ManualSp.address(), 12);
    }
}
